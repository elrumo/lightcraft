//! The real YuNet and SFace models on public-domain portraits. Skipped (passes) unless both folders
//! are given:
//! `LIGHTCRAFT_FACES_DIR=/path/with/the/two/onnx/files LIGHTCRAFT_PORTRAITS_DIR=/path/with/portraits \
//!  cargo test -p lightcraft-vision --features onnx --release --test faces_reference -- --nocapture`
//! The portraits are `jonny_kim_a.jpg`, `jonny_kim_b.jpg` (the same person) and `mae_jemison.jpg`
//! (another), NASA photographs (public domain), never kept in the repository.

use std::path::{Path, PathBuf};
use std::time::Instant;

use lightcraft_raster::{Image, Rgba8};
use lightcraft_vision::Key;
use lightcraft_vision::faces::cluster::{self, JOIN};
use lightcraft_vision::faces::index::FaceIndex;
use lightcraft_vision::faces::model::Faces;
use lightcraft_vision::faces::{FaceEngine, FaceFound};

fn dirs() -> Option<(PathBuf, PathBuf)> {
    Some((std::env::var_os("LIGHTCRAFT_FACES_DIR")?.into(), std::env::var_os("LIGHTCRAFT_PORTRAITS_DIR")?.into()))
}

/// A portrait scaled to at most 1280 px on its long side, as the app renders photos for this.
fn portrait(dir: &Path, name: &str) -> Rgba8 {
    let img = image::open(dir.join(name)).unwrap().to_rgba8();
    let (w, h) = img.dimensions();
    let scale = (1280.0 / w.max(h) as f32).min(1.0);
    let img = image::imageops::resize(&img, (w as f32 * scale) as u32, (h as f32 * scale) as u32, image::imageops::FilterType::Triangle);
    Image { width: img.width() as usize, height: img.height() as usize, data: img.pixels().map(|p| p.0).collect() }
}

fn cos(a: &FaceFound, b: &FaceFound) -> f32 {
    a.embedding.iter().zip(&b.embedding).map(|(x, y)| x * y).sum()
}

#[test]
fn the_same_person_is_closer_than_a_stranger() {
    let Some((models, portraits)) = dirs() else {
        eprintln!("LIGHTCRAFT_FACES_DIR / LIGHTCRAFT_PORTRAITS_DIR not set: skipping");
        return;
    };
    let t = Instant::now();
    let faces = Faces::load(&models).expect("the models load");
    println!("loaded (with warm-up) in {:?}", t.elapsed());
    let mut found = Vec::new();
    for name in ["jonny_kim_a.jpg", "jonny_kim_b.jpg", "mae_jemison.jpg"] {
        let img = portrait(&portraits, name);
        let t = Instant::now();
        let f = faces.faces(&img).unwrap();
        println!(
            "{name}: {}×{}, {} face(s) in {:?}: {:?}",
            img.width,
            img.height,
            f.len(),
            t.elapsed(),
            f.iter().map(|f| (f.rect, f.score)).collect::<Vec<_>>()
        );
        assert_eq!(f.len(), 1, "{name} has one face");
        assert!(f[0].score > 0.85 && (f[0].embedding.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-3);
        let r = f[0].rect;
        assert!(r[0] >= 0.0 && r[1] >= 0.0 && r[2] <= 1.0 && r[3] <= 1.0 && r[2] > r[0] && r[3] > r[1]);
        found.push(f.into_iter().next().unwrap());
    }
    let (same, other_a, other_b) = (cos(&found[0], &found[1]), cos(&found[0], &found[2]), cos(&found[1], &found[2]));
    println!("cosine: same person {same:.3}, strangers {other_a:.3} and {other_b:.3} (SFace's own threshold: 0.363)");
    assert!(same >= 0.363, "the same person: {same}");
    assert!(other_a < 0.363 && other_b < 0.363, "strangers: {other_a}, {other_b}");

    // as the app uses them: stored, then grouped
    let mut ix = FaceIndex::in_memory();
    for (i, f) in found.iter().enumerate() {
        ix.insert_photo(Key::of(&format!("portrait-{i}")), std::slice::from_ref(f)).unwrap();
    }
    let groups = cluster::group(ix.faces(), JOIN);
    println!("grouped: {:?}", groups.iter().map(|g| g.members.len()).collect::<Vec<_>>());
    assert_eq!(groups.iter().map(|g| g.members.len()).collect::<Vec<_>>(), [2, 1]);
}

#[test]
fn photos_without_faces_and_odd_sizes_are_fine() {
    let Some((models, _)) = dirs() else { return };
    let faces = Faces::load(&models).unwrap();
    let blank = |w: usize, h: usize| Image { width: w, height: h, data: vec![[90u8, 120, 60, 255]; w * h] };
    for (w, h) in [(1280, 720), (720, 1280), (64, 64), (1, 1), (3000, 40), (40, 3000)] {
        assert!(faces.faces(&blank(w, h)).unwrap().is_empty(), "{w}×{h}");
    }
    assert!(faces.faces(&Image { width: 0, height: 0, data: vec![] }).is_err());
    assert!(faces.faces(&Image { width: 10, height: 10, data: vec![[0, 0, 0, 255]; 5] }).is_err(), "pixels that don't match the size");
}
