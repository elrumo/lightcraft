//! Colorize commands: the frontend's model gets the photo from `colorize.input` and its colours go back to
//! `colorize.apply`; a grey photo then renders in colour, on and off, Amount and undo are edits like any other, a bad map
//! changes nothing, and the Core ML models are offered only where the frontend says it runs them.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use lightcraft_develop::DevelopSettings;
use serde_json::{Value, json};

use crate::Session;
use crate::catalog::PhotoId;

fn grey_photo() -> (Session, PhotoId, std::path::PathBuf) {
    photo(|v| [v, v, v])
}

/// A scan of a print that is not purely grey: a warm cast on a grey ramp.
fn tinted_photo() -> (Session, PhotoId, std::path::PathBuf) {
    photo(|v| [v.saturating_add(14), v, v.saturating_sub(14)])
}

fn photo(pixel: fn(u8) -> [u8; 3]) -> (Session, PhotoId, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("lc-colorize-{}-{}", std::process::id(), pixel(100)[0]));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("grey.jpg");
    let (w, h) = (96usize, 64usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| 60 + (i % w) as u8).map(pixel).map(|[r, g, b]| [r, g, b, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_jpeg(&lightcraft_codecs::EncodeImage::rgba8(&img), 95, Default::default(), &Default::default()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    let id = r["imported"][0].as_u64().unwrap_or_else(|| panic!("{r}"));
    s.execute("library.select", &json!({"ids": [id], "active": id})).unwrap();
    (s, PhotoId(id), dir)
}

fn dev(s: &Session, id: PhotoId) -> DevelopSettings {
    (*s.develop_of(id).unwrap()).clone()
}

/// How far from grey a render is: the mean of each pixel's largest channel difference.
fn colourfulness(img: &lightcraft_raster::Rgba8) -> f64 {
    let n = img.data.len().max(1) as f64;
    img.data.iter().map(|p| f64::from(p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]))).sum::<f64>() / n
}

/// Mean red minus mean blue: positive for a warm picture.
fn warmth(img: &lightcraft_raster::Rgba8) -> f64 {
    img.data.iter().map(|p| f64::from(p[0]) - f64::from(p[2])).sum::<f64>() / img.data.len().max(1) as f64
}

fn map(w: usize, h: usize, a: i8, b: i8) -> Value {
    let bytes: Vec<u8> = (0..w * h).flat_map(|_| [a as u8, b as u8]).collect();
    json!({"width": w, "height": h, "data": STANDARD.encode(bytes)})
}

#[test]
fn a_models_colours_colour_a_grey_photo_as_an_edit() {
    let (mut s, id, dir) = grey_photo();
    // the model's input: the photo's shape, at most the asked size, a PNG
    let i = s.execute("colorize.input", &json!({"maxEdge": 48})).unwrap();
    assert_eq!((i["width"].as_u64(), i["height"].as_u64(), i["photo"].as_u64()), (Some(48), Some(32), Some(id.0)), "{i}");
    let png = STANDARD.decode(i["png"].as_str().unwrap()).unwrap();
    assert!(png.starts_with(b"\x89PNG"));

    let grey = s.render_now(id, 96, 64).unwrap().image;
    assert!(colourfulness(&grey) < 2.0, "{}", colourfulness(&grey));
    // no colours yet: turning it on says so and changes nothing
    let before = dev(&s, id);
    assert!(s.execute("colorize.apply", &json!({})).unwrap_err().to_string().contains("no colours yet"));
    // a damaged map changes nothing either
    let bad = json!({"width": 3, "height": 2, "data": STANDARD.encode([0u8; 5])});
    assert!(s.execute("colorize.apply", &json!({"map": bad})).is_err());
    assert_eq!(dev(&s, id), before);

    // warm colours (a map of another size than the photo's: it is the photo's shape that counts)
    let r = s.execute("colorize.apply", &json!({"map": map(24, 16, 20, 40), "model": "ddcolor-tiny"})).unwrap();
    assert_eq!((r["enabled"].as_bool(), r["model"].as_str(), r["colors"]["width"].as_u64()), (Some(true), Some("ddcolor-tiny"), Some(24)), "{r}");
    let warm = s.render_now(id, 96, 64).unwrap().image;
    assert!(warmth(&warm) > 15.0 && colourfulness(&warm) > 15.0, "{} {}", warmth(&warm), colourfulness(&warm));
    // its lightness is the photo's
    let luma = |img: &lightcraft_raster::Rgba8| {
        img.data.iter().map(|p| 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])).sum::<f64>() / img.data.len() as f64
    };
    assert!((luma(&warm) - luma(&grey)).abs() < 6.0, "{} {}", luma(&warm), luma(&grey));

    // Amount is a slider: half of it is about half the colour
    s.execute("develop.set", &json!({"control": "colorize.amount", "value": 50})).unwrap();
    let half = s.render_now(id, 96, 64).unwrap().image;
    assert!(warmth(&half) > 0.3 * warmth(&warm) && warmth(&half) < 0.7 * warmth(&warm), "{} {}", warmth(&half), warmth(&warm));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(dev(&s, id).colorize.amount, 100.0);

    // off and on again with the same colours; then undo back to grey
    s.execute("colorize.apply", &json!({"on": false})).unwrap();
    assert_eq!(s.render_now(id, 96, 64).unwrap().image, grey);
    s.execute("colorize.apply", &json!({})).unwrap();
    assert_eq!(s.render_now(id, 96, 64).unwrap().image, warm);
    // the map is the photo's: no copy carries it
    let copy = lightcraft_develop::extract_groups(&dev(&s, id), &lightcraft_develop::SettingsGroup::ALL);
    assert!(copy.get("colorize").is_none(), "{copy}");
    // Remove Colors forgets them, and is undone like any edit
    s.execute("colorize.clear", &json!({})).unwrap();
    assert!(dev(&s, id).colorize.map.is_none());
    assert_eq!(s.render_now(id, 96, 64).unwrap().image, grey);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(dev(&s, id).colorize.map.is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn colours_go_over_the_photo_or_over_its_black_and_white_and_survive_the_bw_treatment() {
    let (mut s, id, dir) = tinted_photo();
    let tinted = s.render_now(id, 96, 64).unwrap().image;
    assert!(colourfulness(&tinted) > 20.0, "{}", colourfulness(&tinted));
    // neutral colours from the model (a = b = 0): what is left is the photo's own tint, or none of it
    let r = s.execute("colorize.apply", &json!({"map": map(8, 8, 0, 0)})).unwrap();
    assert_eq!(r["base"], "color", "{r}");
    let over_colour = s.render_now(id, 96, 64).unwrap().image;
    assert!(colourfulness(&over_colour) > 0.8 * colourfulness(&tinted), "{}", colourfulness(&over_colour));
    // `base` alone changes what they go over, nothing else
    let r = s.execute("colorize.apply", &json!({"base": "bw"})).unwrap();
    assert_eq!((r["base"].as_str(), r["enabled"].as_bool()), (Some("bw"), Some(true)), "{r}");
    let over_bw = s.render_now(id, 96, 64).unwrap().image;
    assert!(colourfulness(&over_bw) < 2.0, "{}", colourfulness(&over_bw));
    assert!(s.execute("colorize.apply", &json!({"base": "sepia"})).is_err());
    assert_eq!(dev(&s, id).colorize.base, lightcraft_develop::ColorizeBase::Bw);
    // warm colours over the black and white version, with the black-and-white treatment on: the colours stay
    s.execute("colorize.apply", &json!({"map": map(8, 8, 20, 40)})).unwrap();
    let warm = s.render_now(id, 96, 64).unwrap().image;
    assert!(warmth(&warm) > 15.0, "{}", warmth(&warm));
    s.execute("develop.treatment", &json!({})).unwrap();
    assert_eq!(dev(&s, id).treatment, lightcraft_develop::Treatment::Bw);
    let with_treatment = s.render_now(id, 96, 64).unwrap().image;
    assert!(warmth(&with_treatment) > 15.0, "{}", warmth(&with_treatment));
    // the choice is the photo's and is kept by Remove Colors, like Amount
    s.execute("colorize.clear", &json!({})).unwrap();
    assert_eq!(dev(&s, id).colorize.base, lightcraft_develop::ColorizeBase::Bw);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_core_ml_models_are_offered_only_where_the_frontend_runs_them() {
    let mut s = Session::with_demo();
    let ids = |s: &mut Session| -> Vec<String> {
        let l = s.execute("models.list", &json!({})).unwrap();
        l["models"].as_array().unwrap().iter().filter(|m| m["available"] == true).filter_map(|m| m["id"].as_str().map(String::from)).collect()
    };
    assert!(!ids(&mut s).iter().any(|i| i.starts_with("ddcolor")));
    if !cfg!(any(target_os = "macos", target_os = "ios")) || !crate::enhance::Enhancer::AVAILABLE {
        return;
    }
    s.enhancer.host_models.insert("ddcolor-tiny".into(), vec!["https://example.com/ddcolor-tiny/".into(), "http://insecure.example".into()]);
    let listed = ids(&mut s);
    assert!(listed.contains(&"ddcolor-tiny".to_string()) && !listed.contains(&"ddcolor-large".to_string()), "{listed:?}");
    // its mirror is the frontend's (only https, without a trailing slash)
    assert_eq!(s.enhancer.mirrors("ddcolor-tiny"), vec!["https://example.com/ddcolor-tiny".to_string()]);
}
