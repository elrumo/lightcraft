//! Search by description with the deterministic stand-in model (no weights): indexing, ranking,
//! persistence, failures that must not take the session down.

use std::sync::Arc;
use std::time::{Duration, Instant};

use lightcraft_raster::Rgba8;
use lightcraft_vision::fake::FakeEmbedder;
use lightcraft_vision::{Embedder, Error};
use serde_json::{Value, json};

use crate::Session;

fn demo() -> Session {
    let mut s = Session::with_demo();
    s.vision.set_embedder(Arc::new(FakeEmbedder));
    s
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-vision-engine-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn ids(v: &Value) -> Vec<u64> {
    v["photos"].as_array().unwrap().iter().map(|p| p["id"].as_u64().unwrap()).collect()
}

#[test]
fn photos_are_indexed_once_and_the_count_is_reported() {
    let mut s = demo();
    let before = s.execute("vision.model.status", &json!({})).unwrap();
    assert_eq!((before["available"].clone(), before["installed"].clone()), (json!(true), json!(true)));
    assert_eq!(before["indexed"], Value::Null, "not opened yet");

    let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
    let keys = s.vision_keys().len();
    assert!(keys > 0);
    assert_eq!(
        (r["total"].as_u64(), r["done"].as_u64(), r["failed"].as_u64(), r["running"].clone()),
        (Some(keys as u64), Some(keys as u64), Some(0), json!(false)),
        "{r}"
    );
    assert_eq!(s.vision.indexed(), keys);

    // nothing left to do the second time
    let again = s.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!(again["total"], 0, "{again}");
    let st = s.execute("vision.model.status", &json!({})).unwrap();
    assert_eq!(st["indexed"], json!(keys));
    assert_eq!(s.execute("vision.indexProgress", &json!({})).unwrap()["total"], 0);
}

#[test]
fn a_description_shows_the_best_matches_first() {
    let mut s = demo();
    s.execute("vision.index", &json!({"wait": true})).unwrap();
    let all = s.visible_cloned().len();

    let red = s.execute("library.search", &json!({"q": "a red sunset", "wait": true})).unwrap();
    let red_ids = ids(&red);
    assert!(!red_ids.is_empty() && red_ids.len() <= 200);
    let scores: Vec<f64> = red["photos"].as_array().unwrap().iter().map(|p| p["score"].as_f64().unwrap()).collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "best first: {scores:?}");

    // it is the view's filter, in rank order, with a chip that undoes it
    assert_eq!(s.filter.semantic.as_deref(), Some("a red sunset"));
    assert_eq!(s.visible_cloned().iter().map(|i| i.0).collect::<Vec<_>>(), red_ids);
    let chips = crate::view::filter_chips(&s.filter, &s.catalog);
    assert_eq!(chips.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["Looks like: a red sunset"]);
    assert_eq!(chips[0].clear, json!({"semantic": null, "only": []}));
    // another description ranks differently
    let blue = ids(&s.execute("library.search", &json!({"q": "blue", "wait": true})).unwrap());
    assert_ne!(red_ids, blue);
    assert_eq!(s.visible_cloned().iter().map(|i| i.0).collect::<Vec<_>>(), blue);

    // a limit
    let few = ids(&s.execute("library.search", &json!({"q": "blue", "limit": 3, "wait": true})).unwrap());
    assert_eq!(few, blue[..3]);

    // clearing the search brings everything back
    s.execute("library.filter", &json!({"semantic": null, "only": []})).unwrap();
    assert_eq!(s.filter.semantic, None);
    assert_eq!(s.visible_cloned().len(), all);
}

#[test]
fn a_search_with_no_matches_shows_nothing_not_everything() {
    let mut s = demo();
    s.execute("vision.index", &json!({"wait": true})).unwrap();
    s.filter.semantic = Some("nothing".into());
    s.filter.only.clear();
    assert!(s.visible_cloned().is_empty());
    // while a plain empty `only` is still "no constraint"
    s.filter.semantic = None;
    assert!(!s.visible_cloned().is_empty());
}

#[test]
fn search_needs_a_query_an_index_and_a_model() {
    let mut s = demo();
    let e = s.execute("library.search", &json!({"q": "red", "wait": true})).unwrap_err().to_string();
    assert!(e.contains("vision.index"), "no photos are indexed yet: {e}");
    s.execute("vision.index", &json!({"wait": true})).unwrap();
    for q in [json!({"wait": true}), json!({"q": "   ", "wait": true}), json!({"q": "", "wait": true})] {
        assert!(s.execute("library.search", &q).is_err(), "{q}");
    }
    // a query far longer than anything useful is cut, not refused
    let long = "red ".repeat(10_000);
    assert!(s.execute("library.search", &json!({"q": long, "wait": true})).is_ok());
    assert!(s.filter.semantic.as_ref().unwrap().chars().count() <= crate::vision::MAX_QUERY);

    // no model at all (this build has none, and none was supplied)
    let mut plain = Session::with_demo();
    assert_eq!(plain.execute("vision.model.status", &json!({})).unwrap()["available"], json!(cfg!(feature = "vision")));
    if !cfg!(feature = "vision") {
        let e = plain.execute("library.search", &json!({"q": "red", "wait": true})).unwrap_err().to_string();
        assert!(e.contains("not available"), "{e}");
        assert!(plain.execute("vision.index", &json!({"wait": true})).is_err());
        assert!(plain.execute("vision.model.download", &json!({"acknowledged": true})).is_err());
    }
}

#[test]
fn downloading_the_model_needs_the_users_agreement() {
    let mut s = demo();
    let e = s.execute("vision.model.download", &json!({})).unwrap_err().to_string();
    assert!(e.contains("acknowledged") && e.contains("Apache"), "{e}");
    let e = s.execute("vision.model.download", &json!({"acknowledged": false})).unwrap_err().to_string();
    assert!(e.contains("acknowledged"), "{e}");
    // installed already (a model was supplied): nothing to download
    assert_eq!(s.execute("vision.model.download", &json!({"acknowledged": true})).unwrap()["started"], json!(false));
}

#[test]
fn the_index_survives_a_restart_and_is_not_redone() {
    let dir = temp_dir("persist");
    let mut s = Session::new();
    s.open_library(&dir, true).unwrap();
    s.vision.set_embedder(Arc::new(FakeEmbedder));
    s.execute("vision.index", &json!({"wait": true})).unwrap();
    let keys = s.vision_keys().len();
    let file = dir.join("search").join("fake-colour.bin");
    assert!(file.is_file(), "{file:?}");
    let first = ids(&s.execute("library.search", &json!({"q": "green", "wait": true})).unwrap());
    s.close_library().ok();
    drop(s);

    let mut s2 = Session::new();
    s2.open_library(&dir, true).unwrap();
    s2.vision.set_embedder(Arc::new(FakeEmbedder));
    let again = s2.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!(again["total"], 0, "everything was already indexed: {again}");
    assert_eq!(s2.vision.indexed(), keys);
    assert_eq!(ids(&s2.execute("library.search", &json!({"q": "green", "wait": true})).unwrap()), first);

    // another model keeps its own index file
    s2.vision.set_embedder(Arc::new(Other));
    s2.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!(s2.vision.indexed(), keys);
    assert!(dir.join("search").join("other-model.bin").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_index_file_is_started_over() {
    let dir = temp_dir("damaged");
    let mut s = Session::new();
    s.open_library(&dir, true).unwrap();
    s.vision.set_embedder(Arc::new(FakeEmbedder));
    std::fs::create_dir_all(dir.join("search")).unwrap();
    std::fs::write(dir.join("search").join("fake-colour.bin"), vec![9u8; 500]).unwrap();
    let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!(r["failed"], 0, "{r}");
    assert_eq!(s.vision.indexed(), s.vision_keys().len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn in_the_app_searching_and_indexing_run_in_the_background() {
    let mut s = demo();
    s.vision.background = true;
    let r = s.execute("vision.index", &json!({})).unwrap();
    assert!(r["total"].as_u64().unwrap() > 0);
    let wait_for = |s: &mut Session, what: &str, mut done: Box<dyn FnMut(&mut Session) -> bool>| {
        let end = Instant::now() + Duration::from_secs(120);
        while !done(s) {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    wait_for(&mut s, "the index", Box::new(|s| s.vision.job().is_some_and(|j| j.finished.load(std::sync::atomic::Ordering::Relaxed))));
    let polled = s.vision_poll();
    assert!(polled.messages.iter().any(|m| m.starts_with("Indexed ")), "{polled:?}");
    assert!(s.vision_poll().messages.is_empty(), "each notice once");

    let r = s.execute("library.search", &json!({"q": "blue"})).unwrap();
    assert_eq!(r["status"], "searching");
    assert!(s.filter.semantic.is_none(), "not applied until it is done");
    let mut changed = false;
    wait_for(
        &mut s,
        "the search",
        Box::new(move |s| {
            changed |= s.vision_poll().changed;
            changed
        }),
    );
    assert_eq!(s.filter.semantic.as_deref(), Some("blue"));
    assert!(!s.filter.only.is_empty());
    assert!(!s.vision.searching());
}

#[test]
fn only_the_newest_search_is_applied() {
    let mut s = demo();
    s.execute("vision.index", &json!({"wait": true})).unwrap();
    s.vision.background = true;
    s.execute("library.search", &json!({"q": "red"})).unwrap();
    s.execute("library.search", &json!({"q": "green"})).unwrap();
    let end = Instant::now() + Duration::from_secs(120);
    while s.vision.searching() {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    s.vision_poll();
    assert_eq!(s.filter.semantic.as_deref(), Some("green"));
}

/// A model that works but is named differently, to find an index for another model.
struct Other;

impl Embedder for Other {
    fn model_id(&self) -> &str {
        "other-model"
    }
    fn dim(&self) -> usize {
        4
    }
    fn encode_text(&self, t: &str) -> Result<Vec<f32>, Error> {
        FakeEmbedder.encode_text(t)
    }
    fn encode_images(&self, i: &[&Rgba8]) -> Result<Vec<Vec<f32>>, Error> {
        FakeEmbedder.encode_images(i)
    }
}

/// A model that panics, or fails.
struct Broken {
    panic: bool,
}

impl Embedder for Broken {
    fn model_id(&self) -> &str {
        "broken"
    }
    fn dim(&self) -> usize {
        4
    }
    fn encode_text(&self, _: &str) -> Result<Vec<f32>, Error> {
        if self.panic { panic!("text tower exploded") } else { Err(Error::Model("no text today".into())) }
    }
    fn encode_images(&self, _: &[&Rgba8]) -> Result<Vec<Vec<f32>>, Error> {
        if self.panic { panic!("vision tower exploded") } else { Err(Error::Model("no images today".into())) }
    }
}

#[test]
fn a_failing_or_panicking_model_is_an_error_not_a_crash() {
    for panic in [false, true] {
        let mut s = Session::with_demo();
        s.vision.set_embedder(Arc::new(Broken { panic }));
        let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
        assert_eq!(r["running"], json!(false), "the run ends: {r}");
        assert!(r["error"].as_str().is_some_and(|e| e.contains("today") || e.contains("exploded")), "{r}");
        assert_eq!(r["failed"], r["total"], "{r}");
        assert_eq!(s.vision.indexed(), 0);
        // and the session carries on
        assert!(s.execute("library.search", &json!({"q": "x", "wait": true})).is_err());
        assert!(s.execute("photo.rate", &json!({"rating": 3})).is_ok());
        // a new run is allowed after a failed one
        assert!(s.execute("vision.index", &json!({"wait": true})).is_ok());
    }
}

#[test]
fn a_panic_while_searching_is_an_error() {
    let mut s = demo();
    s.execute("vision.index", &json!({"wait": true})).unwrap();
    s.vision.set_embedder(Arc::new(PanicsOnText));
    let e = s.execute("library.search", &json!({"q": "red", "wait": true})).unwrap_err().to_string();
    assert!(e.contains("failed unexpectedly") && e.contains("boom"), "{e}");
    assert!(s.execute("photo.rate", &json!({"rating": 2})).is_ok());
}

/// The fake's index, but its text tower panics.
struct PanicsOnText;

impl Embedder for PanicsOnText {
    fn model_id(&self) -> &str {
        FakeEmbedder.model_id()
    }
    fn dim(&self) -> usize {
        FakeEmbedder.dim()
    }
    fn encode_text(&self, _: &str) -> Result<Vec<f32>, Error> {
        panic!("boom")
    }
    fn encode_images(&self, i: &[&Rgba8]) -> Result<Vec<Vec<f32>>, Error> {
        FakeEmbedder.encode_images(i)
    }
}

#[test]
fn indexing_can_be_cancelled() {
    let mut s = demo();
    s.vision.background = true;
    s.execute("vision.index", &json!({})).unwrap();
    s.execute("vision.indexCancel", &json!({})).unwrap();
    let end = Instant::now() + Duration::from_secs(120);
    while !s.vision.job().unwrap().finished.load(std::sync::atomic::Ordering::Relaxed) {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    let p = s.execute("vision.indexProgress", &json!({})).unwrap();
    assert_eq!(p["cancelled"], json!(true));
    // and indexing can start again
    s.vision.background = false;
    s.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!(s.vision.indexed(), s.vision_keys().len());
}
