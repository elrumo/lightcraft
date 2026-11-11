//! Search by colour: "red" finds the photos that are mostly red, with no model at all; "blue sky" keeps the
//! description's matches to the blue ones. The colours are worked out in the pass that indexes the photos.

use std::path::Path;
use std::sync::Arc;

use lightcraft_raster::Rgba8;
use lightcraft_vision::fake::FakeEmbedder;
use serde_json::{Value, json};

use crate::Session;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-colors-engine-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, f: impl Fn(usize, usize) -> [u8; 3]) {
    let (w, h) = (80usize, 60usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| f(i % w, i / w)).map(|[r, g, b]| [r, g, b, 255]).collect();
    let img = Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

const RED: [u8; 3] = [220, 30, 30];
const GREEN: [u8; 3] = [30, 190, 60];
const BLUE: [u8; 3] = [30, 70, 220];
const GREY: [u8; 3] = [128, 128, 128];

/// A library of five photos (named so no colour word is in a file name): all red, all green, all blue, half green and
/// half blue, grey. → the session, its folder, and the photo ids in that order.
fn library(tag: &str) -> (Session, std::path::PathBuf, [u64; 5]) {
    let (src, lib) = (temp_dir(&format!("{tag}-src")), temp_dir(&format!("{tag}-lib")));
    write_png(&src.join("p1.png"), |_, _| RED);
    write_png(&src.join("p2.png"), |_, _| GREEN);
    write_png(&src.join("p3.png"), |_, _| BLUE);
    write_png(&src.join("p4.png"), |x, _| if x < 40 { GREEN } else { BLUE });
    write_png(&src.join("p5.png"), |_, _| GREY);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    let id = |name: &str| s.catalog.photos().find(|p| p.file_name == name).map(|p| p.id.0).unwrap();
    let ids = [id("p1.png"), id("p2.png"), id("p3.png"), id("p4.png"), id("p5.png")];
    (s, lib, ids)
}

fn found(v: &Value) -> Vec<u64> {
    v["photos"].as_array().unwrap().iter().map(|p| p["id"].as_u64().unwrap()).collect()
}

#[test]
fn a_colour_word_finds_the_photos_that_are_mostly_that_colour_with_no_model() {
    let (mut s, _, [red, green, blue, half, grey]) = library("words");
    // before anything is worked out, a colour word is just a word: nothing to search by, no model to search with
    assert!(s.execute("library.search", &json!({"q": "red", "wait": true})).is_err());
    // no model here, yet indexing works the colours out
    let run = s.execute("vision.index", &json!({"wait": true})).unwrap();
    // (each photo's colours are a step of the run's bar, HB-331; no model's work in it)
    assert_eq!(run["total"], 5, "{run}");
    assert_eq!(s.vision_colors_len(), 5);

    let r = s.execute("library.search", &json!({"q": "red", "wait": true})).unwrap();
    assert_eq!(found(&r), [red], "{r}");
    assert_eq!(r["colors"], json!(["red"]));
    assert_eq!((r["photos"][0]["color"].clone(), r["photos"][0]["text"].clone()), (json!(true), json!(false)));
    // it is the view's filter like any search
    assert_eq!(s.visible_cloned().iter().map(|i| i.0).collect::<Vec<_>>(), [red]);

    // all of a colour first, then the photo that is half of it
    assert_eq!(found(&s.execute("library.search", &json!({"q": "blue", "wait": true})).unwrap()), [blue, half]);
    assert_eq!(found(&s.execute("library.search", &json!({"q": "Green", "wait": true})).unwrap()), [green, half]);
    // every colour named: only the photo with both
    let both = s.execute("library.search", &json!({"q": "green and blue", "wait": true})).unwrap();
    assert_eq!(found(&both), [half], "{both}");
    assert_eq!(both["colors"], json!(["green", "blue"]));
    assert_eq!(found(&s.execute("library.search", &json!({"q": "grey", "wait": true})).unwrap()), [grey]);
    assert_eq!(found(&s.execute("library.search", &json!({"q": "gris", "wait": true})).unwrap()), [grey], "Spanish too");
    // a colour no photo is gives an empty view, not everything
    let none = s.execute("library.search", &json!({"q": "purple", "wait": true})).unwrap();
    assert!(found(&none).is_empty() && s.visible_cloned().is_empty(), "{none}");
    // a limit holds
    assert_eq!(found(&s.execute("library.search", &json!({"q": "blue", "limit": 1, "wait": true})).unwrap()), [blue]);
    // a colour and something only a model can answer, with no model: it says so
    assert!(s.execute("library.search", &json!({"q": "red car", "wait": true})).is_err());
}

#[test]
fn colour_and_description_together_bring_the_photos_with_the_colour_to_the_front() {
    let (mut s, _, [red, green, blue, half, grey]) = library("both");
    s.vision.set_embedder(Arc::new(FakeEmbedder));
    s.execute("vision.index", &json!({"wait": true})).unwrap();
    let r = s.execute("library.search", &json!({"q": "blue sky", "wait": true})).unwrap();
    let ids = found(&r);
    assert_eq!(ids.len(), 5, "the others follow: {r}");
    let mut first = ids[..2].to_vec();
    first.sort_unstable();
    assert_eq!(first, [blue.min(half), blue.max(half)], "{r}");
    let flags: Vec<bool> = r["photos"].as_array().unwrap().iter().map(|p| p["color"].as_bool().unwrap()).collect();
    assert_eq!(flags, [true, true, false, false, false], "{r}");
    assert_eq!(r["colors"], json!(["blue"]));
    // a colour word alone, with a model: the photos with the colour, most of it first, then what the model finds
    let r = s.execute("library.search", &json!({"q": "greenish", "wait": true})).unwrap();
    let ids = found(&r);
    assert_eq!(ids[..2], [green, half], "{r}");
    assert_eq!(ids.len(), 5);
    assert_eq!(r["photos"][2]["color"], false);
    // no colour word: the description alone, none of it for a colour
    let r = s.execute("library.search", &json!({"q": "sky", "wait": true})).unwrap();
    assert_eq!((found(&r).len(), r["colors"].clone()), (5, json!([])));
    let _ = (red, grey);
}

#[test]
fn photos_already_described_get_their_colours_without_being_described_again() {
    let (mut s, lib, _) = library("catch-up");
    s.vision.set_embedder(Arc::new(FakeEmbedder));
    let first = s.execute("vision.index", &json!({"wait": true})).unwrap();
    assert_eq!(first["total"], 5);
    assert_eq!(s.vision_colors_len(), 5, "worked out in the same pass");
    s.close_library().ok();
    drop(s);

    // a library indexed before there were colours: the descriptions are there, the colours are not
    let file = lib.join("search").join(format!("colors-{}.bin", lightcraft_vision::colors::ENGINE));
    assert!(file.is_file());
    std::fs::remove_file(&file).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.vision.set_embedder(Arc::new(FakeEmbedder));
    assert_eq!(s.vision_colors_len(), 0);
    let again = s.execute("vision.index", &json!({"wait": true})).unwrap();
    // (nothing is described again: the five steps are the colours alone)
    assert_eq!(again["total"], 5, "{again}");
    assert_eq!(s.vision_colors_len(), 5);
    assert_eq!(s.vision.indexed(), 5);
    assert!(s.execute("library.search", &json!({"q": "red", "wait": true})).is_ok());
    s.close_library().ok();
}

#[test]
fn processing_again_forgets_only_the_colours() {
    let (mut s, lib, [red, ..]) = library("again");
    s.vision.set_embedder(Arc::new(FakeEmbedder));
    s.execute("vision.index", &json!({"wait": true})).unwrap();
    let file = lib.join("search").join(format!("colors-{}.bin", lightcraft_vision::colors::ENGINE));
    let before = std::fs::metadata(&file).unwrap().len();
    let r = s.execute("vision.reprocess", &json!({"part": "colors", "scope": "device"})).unwrap();
    assert_eq!((r["part"].clone(), r["device"].clone(), r["indexing"].clone()), (json!("colors"), json!(true), json!(true)), "{r}");
    // (no photo is described again: the five steps are the colours alone)
    assert_eq!(s.vision.job().map(|j| j.total), Some(5));
    assert_eq!(std::fs::metadata(&file).unwrap().len(), before, "made again");
    assert_eq!(found(&s.execute("library.search", &json!({"q": "red", "wait": true})).unwrap())[0], red);
    assert!(s.execute("vision.reprocess", &json!({"part": "colours", "scope": "device"})).is_err());
}

#[test]
fn a_damaged_colours_file_is_started_over_not_a_crash() {
    let (mut s, lib, [red, ..]) = library("damaged");
    s.execute("vision.index", &json!({"wait": true})).unwrap();
    s.close_library().ok();
    drop(s);
    let file = lib.join("search").join(format!("colors-{}.bin", lightcraft_vision::colors::ENGINE));
    // garbage, a header cut short, and a good file with a record cut in the middle
    let good = std::fs::read(&file).unwrap();
    for junk in [vec![0xffu8; 200], good[..11].to_vec(), good[..good.len() - 7].to_vec(), Vec::new()] {
        std::fs::write(&file, &junk).unwrap();
        let mut s = Session::new().with_fs();
        s.open_library(&lib, false).unwrap();
        let _ = s.execute("library.search", &json!({"q": "red", "wait": true}));
        s.execute("vision.index", &json!({"wait": true})).unwrap();
        assert_eq!(found(&s.execute("library.search", &json!({"q": "red", "wait": true})).unwrap()), [red]);
        s.close_library().ok();
    }
}

#[test]
fn the_colour_names_a_front_end_can_suggest_are_listed() {
    let mut s = Session::with_demo();
    let r = s.execute("library.colorNames", &json!({})).unwrap();
    let names: Vec<&str> = r["colors"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 12);
    assert!(names.contains(&"red") && names.contains(&"grey") && r["colors"][0]["rgb"].as_array().unwrap().len() == 3);
    assert!(r["words"].as_array().unwrap().iter().any(|w| w["word"] == "rojo" && w["color"] == "red"));
    assert_eq!(r["analysed"], 0);
}
