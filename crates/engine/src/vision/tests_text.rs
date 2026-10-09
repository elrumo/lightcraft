//! Searching the text printed in photos, with the deterministic stand-in reader (no models): when
//! it is read, how it ranks against the description search, and failures that must not take the
//! session down.

use std::sync::Arc;

use lightcraft_raster::Rgba8;
use lightcraft_vision::fake::{FakeEmbedder, FakeReader};
use lightcraft_vision::{Error, TextReader};
use serde_json::{Value, json};

use crate::Session;

fn demo() -> Session {
    let mut s = Session::with_demo();
    s.vision.set_embedder(Arc::new(FakeEmbedder));
    s.vision.set_text_reader(Arc::new(FakeReader));
    s.vision.text_edge = 192;
    s
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-vision-text-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// `(id, found by its text)` for each photo of a search result, best first.
fn found(v: &Value) -> Vec<(u64, bool)> {
    v["photos"].as_array().unwrap().iter().map(|p| (p["id"].as_u64().unwrap(), p["text"].as_bool().unwrap())).collect()
}

fn text_status(s: &mut Session) -> Value {
    s.execute("vision.model.status", &json!({})).unwrap()["text"].clone()
}

#[test]
fn text_is_read_only_after_the_user_turns_it_on() {
    let mut s = demo();
    assert!(!s.vision.text, "off by default");
    let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
    let photos = s.vision_keys().len();
    assert_eq!(r["total"], json!(photos), "only describing: {r}");
    assert_eq!(text_status(&mut s)["indexed"], Value::Null, "no text index was opened");

    // asking for words does nothing while it is off
    let r = s.execute("library.search", &json!({"q": "open hours", "wait": true})).unwrap();
    assert!(found(&r).iter().all(|(_, text)| !text), "{r}");

    assert_eq!(s.execute("vision.setText", &json!({"on": true})).unwrap()["enabled"], json!(true));
    let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!(
        (r["total"].as_u64(), r["failed"].as_u64(), r["error"].clone()),
        (Some(photos as u64), Some(0), Value::Null),
        "only reading is left: {r}"
    );
    assert_eq!(r["phase"], "reading");
    assert_eq!(s.vision.text_indexed(), photos);
    let st = text_status(&mut s);
    assert_eq!((st["enabled"].clone(), st["installed"].clone(), st["indexed"].clone()), (json!(true), json!(true), json!(photos)), "{st}");

    // nothing left to read the second time
    assert_eq!(s.execute("vision.index", &json!({"wait": true})).unwrap()["total"], 0);
}

#[test]
fn words_in_photos_come_before_look_alikes() {
    let mut s = demo();
    s.execute("vision.setText", &json!({"on": true})).unwrap();
    s.execute("vision.index", &json!({"wait": true})).unwrap();
    // the stand-in reader sees a stop sign in a red-dominated photo, and the stand-in model sees red in "red"
    let r = s.execute("library.search", &json!({"q": "stop", "wait": true})).unwrap();
    let hits = found(&r);
    assert!(hits.iter().any(|(_, text)| *text), "a photo that says STOP: {r}");
    let first_look = hits.iter().position(|(_, text)| !text).unwrap_or(hits.len());
    assert!(hits[..first_look].iter().all(|(_, text)| *text) && hits[first_look..].iter().all(|(_, text)| !text), "text matches first: {hits:?}");
    let scores: Vec<f64> = r["photos"].as_array().unwrap().iter().map(|p| p["score"].as_f64().unwrap()).collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "best first: {scores:?}");
    assert!(scores.iter().zip(&hits).all(|(sc, (_, text))| (*sc >= 2.0) == *text), "{scores:?}");

    // the view is the result, in order, and a photo appears once
    let ids: Vec<u64> = hits.iter().map(|h| h.0).collect();
    assert_eq!(s.visible_cloned().iter().map(|i| i.0).collect::<Vec<_>>(), ids);
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len());

    // text is matched forgivingly: case, and one wrong letter in a long word
    let by_text = |r: &Value| found(r).into_iter().filter(|h| h.1).map(|h| h.0).collect::<std::collections::BTreeSet<_>>();
    let exact = s.execute("library.search", &json!({"q": "open hours", "wait": true})).unwrap();
    let typo = s.execute("library.search", &json!({"q": "OPEN hoours", "wait": true})).unwrap();
    assert!(!by_text(&exact).is_empty(), "{exact}");
    assert_eq!(by_text(&exact), by_text(&typo), "the same photos, whatever the case and a slip of the keyboard");

    // switching it off hides the words again
    s.execute("vision.setText", &json!({"on": false})).unwrap();
    let off = s.execute("library.search", &json!({"q": "stop", "wait": true})).unwrap();
    assert!(found(&off).iter().all(|h| !h.1), "{off}");
}

#[test]
fn text_can_be_searched_without_the_description_model() {
    let mut s = Session::with_demo();
    s.vision.set_text_reader(Arc::new(FakeReader));
    s.vision.text_edge = 192;
    s.execute("vision.setText", &json!({"on": true})).unwrap();
    let e = s.execute("library.search", &json!({"q": "stop", "wait": true})).unwrap_err().to_string();
    assert!(!e.is_empty(), "nothing is indexed yet, and there is no description model: {e}");
    // (a build without the description model can still read text)
    let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!(r["error"], Value::Null, "{r}");
    assert!(s.vision.text_indexed() > 0);
    assert_eq!(s.vision.indexed(), 0, "nothing was described");
    let r = s.execute("library.search", &json!({"q": "stop", "wait": true})).unwrap();
    let hits = found(&r);
    assert!(!hits.is_empty() && hits.iter().all(|h| h.1), "{r}");
    assert_eq!(s.filter.semantic.as_deref(), Some("stop"));
    // a word that isn't in any photo finds nothing, and says so with an empty view
    let none = s.execute("library.search", &json!({"q": "xylophone", "wait": true})).unwrap();
    assert!(found(&none).is_empty(), "{none}");
    assert!(s.visible_cloned().is_empty());
}

#[test]
fn the_choice_and_what_was_read_survive_a_restart() {
    let dir = temp_dir("persist");
    let mut s = Session::new();
    s.open_library(&dir, true).unwrap();
    s.vision.set_embedder(Arc::new(FakeEmbedder));
    s.vision.set_text_reader(Arc::new(FakeReader));
    s.vision.text_edge = 192;
    s.execute("vision.setText", &json!({"on": true})).unwrap();
    s.execute("vision.index", &json!({"wait": true})).unwrap();
    let read = s.vision.text_indexed();
    assert!(read > 0);
    let file = dir.join("search").join("text-fake-reader.bin");
    assert!(file.is_file(), "{file:?}");
    let first = found(&s.execute("library.search", &json!({"q": "stop", "wait": true})).unwrap());
    s.close_library().ok();
    drop(s);

    let mut again = Session::new();
    again.open_library(&dir, true).unwrap();
    assert!(again.vision.text, "the choice was saved with the library");
    again.vision.set_embedder(Arc::new(FakeEmbedder));
    again.vision.set_text_reader(Arc::new(FakeReader));
    again.vision.text_edge = 192;
    let r = again.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!(r["total"], 0, "nothing is read twice: {r}");
    assert_eq!(found(&again.execute("library.search", &json!({"q": "stop", "wait": true})).unwrap()), first);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_text_index_is_started_over() {
    let dir = temp_dir("damaged");
    let mut s = Session::new();
    s.open_library(&dir, true).unwrap();
    s.vision.set_text_reader(Arc::new(FakeReader));
    s.vision.text_edge = 192;
    s.execute("vision.setText", &json!({"on": true})).unwrap();
    std::fs::create_dir_all(dir.join("search")).unwrap();
    std::fs::write(dir.join("search").join("text-fake-reader.bin"), vec![7u8; 300]).unwrap();
    let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!((r["failed"].clone(), r["error"].clone()), (json!(0), Value::Null), "{r}");
    assert_eq!(s.vision.text_indexed(), s.vision_keys().len());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A reader that fails, or panics, on every photo.
struct Broken {
    panic: bool,
}

impl TextReader for Broken {
    fn engine(&self) -> &str {
        "broken-reader"
    }
    fn text(&self, _: &Rgba8) -> Result<String, Error> {
        if self.panic { panic!("ocr exploded") } else { Err(Error::Model("no reading today".into())) }
    }
}

#[test]
fn a_failing_or_panicking_reader_is_an_error_not_a_crash() {
    for panic in [false, true] {
        let mut s = Session::with_demo();
        s.vision.set_embedder(Arc::new(FakeEmbedder));
        s.vision.set_text_reader(Arc::new(Broken { panic }));
        s.vision.text_edge = 192;
        s.execute("vision.setText", &json!({"on": true})).unwrap();
        let r = s.execute("vision.index", &json!({"wait": true})).unwrap();
        let photos = s.vision_keys().len();
        // the photos were described; reading gave up after a few failures in a row
        assert_eq!(s.vision.indexed(), photos);
        assert_eq!(r["running"], json!(false), "the run ends: {r}");
        assert!(r["error"].as_str().is_some_and(|e| e.contains("stopped") && (e.contains("today") || e.contains("exploded"))), "{r}");
        assert_eq!(r["failed"], json!(photos), "every photo's reading failed: {r}");
        assert_eq!(s.vision.text_indexed(), 0);
        // and the session carries on
        assert!(s.execute("library.search", &json!({"q": "red", "wait": true})).is_ok());
        assert!(s.execute("photo.rate", &json!({"rating": 3})).is_ok());
    }
}

#[test]
fn downloading_the_text_models_needs_the_users_agreement() {
    let mut s = demo();
    let e = s.execute("vision.text.download", &json!({})).unwrap_err().to_string();
    assert!(e.contains("acknowledged") && e.contains("Apache"), "{e}");
    // installed already (a reader was supplied): nothing to download
    assert_eq!(s.execute("vision.text.download", &json!({"acknowledged": true})).unwrap()["started"], json!(false));
    assert_eq!(s.execute("vision.text.cancel", &json!({})).unwrap()["cancelled"], json!(false));

    let mut plain = Session::with_demo();
    let st = text_status(&mut plain);
    assert_eq!((st["installed"].clone(), st["enabled"].clone()), (json!(false), json!(false)), "{st}");
    if !cfg!(feature = "vision") {
        assert!(plain.execute("vision.text.download", &json!({"acknowledged": true})).is_err());
    }
}
