//! The real SigLIP 2 model on small procedural images. Skipped (passes) unless the model folder is
//! given: `LIGHTCRAFT_VISION_DIR=/path/to/siglip2 cargo test -p lightcraft-vision --release --test reference`.
//! The folder holds `model.safetensors` and `tokenizer.json` from `google/siglip2-base-patch16-224`.

use candle_core::Device;
use lightcraft_raster::{Image, Rgba8};
use lightcraft_vision::siglip::{DIM, MODEL_ID, SigLip};
use lightcraft_vision::{EmbeddingIndex, Key};

fn model() -> Option<SigLip> {
    let dir = std::env::var_os("LIGHTCRAFT_VISION_DIR")?;
    // the CPU, so the result doesn't depend on the machine's GPU
    Some(SigLip::load_on(std::path::Path::new(&dir), Device::Cpu).expect("the model folder loads"))
}

fn flat(px: [u8; 4]) -> Rgba8 {
    Image { width: 256, height: 192, data: vec![px; 256 * 192] }
}

fn checkerboard() -> Rgba8 {
    let data = (0..256 * 192).map(|i| if ((i % 256) / 32 + (i / 256) / 32) % 2 == 0 { [0, 0, 0, 255] } else { [255, 255, 255, 255] }).collect();
    Image { width: 256, height: 192, data }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    dot / (a.iter().map(|x| x * x).sum::<f32>().sqrt() * b.iter().map(|x| x * x).sum::<f32>().sqrt())
}

#[test]
fn captions_find_their_images() {
    let Some(model) = model() else {
        eprintln!("LIGHTCRAFT_VISION_DIR not set: skipping");
        return;
    };
    let shots = [
        ("a red image", flat([220, 20, 20, 255])),
        ("a green image", flat([20, 200, 40, 255])),
        ("a blue image", flat([20, 40, 220, 255])),
        ("a black and white checkerboard", checkerboard()),
    ];
    let texts: Vec<Vec<f32>> = shots.iter().map(|(c, _)| model.encode_text(c).unwrap()).collect();
    let refs: Vec<&Rgba8> = shots.iter().map(|(_, i)| i).collect();
    let images = model.encode_images(&refs).unwrap();
    assert_eq!(images.len(), shots.len());
    assert!(images.iter().all(|v| v.len() == DIM) && texts.iter().all(|v| v.len() == DIM));

    // each image is closest to its own caption
    for (i, v) in images.iter().enumerate() {
        let scores: Vec<f32> = texts.iter().map(|t| cosine(v, t)).collect();
        let best = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(best, i, "{}: scores {scores:?}", shots[i].0);
    }

    // a batch gives the same vector as one by one
    let alone = model.encode_image(&shots[1].1).unwrap();
    assert!(cosine(&alone, &images[1]) > 0.9999);
}

#[test]
fn text_is_case_insensitive_and_long_text_is_cut_not_refused() {
    let Some(model) = model() else {
        return;
    };
    let a = model.encode_text("a photo of a dog on a beach").unwrap();
    let b = model.encode_text("A PHOTO OF A DOG ON A BEACH").unwrap();
    assert!(cosine(&a, &b) > 0.9999);
    assert_ne!(model.encode_text("a photo of a cat").unwrap(), a);
    // far longer than the 64 tokens the model reads, and far longer than the query cap
    let long = "a very long description of a sunset over the sea ".repeat(200);
    assert_eq!(model.encode_text(&long).unwrap().len(), DIM);
    // multilingual: the same idea in Japanese and Chinese lands near the English one
    let en = model.encode_text("a cat").unwrap();
    assert!(cosine(&en, &model.encode_text("猫").unwrap()) > cosine(&en, &model.encode_text("犬").unwrap()));
    assert!(cosine(&en, &model.encode_text("猫的照片").unwrap()) > cosine(&en, &model.encode_text("狗的照片").unwrap()));
    assert_eq!(model.encode_text("").unwrap().len(), DIM);
}

#[test]
fn a_sentence_finds_its_photo_through_the_index() {
    let Some(model) = model() else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("lc-vision-ref-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut index = EmbeddingIndex::open(&dir.join("e.bin"), MODEL_ID, DIM).unwrap();
    let shots = [("red", flat([220, 20, 20, 255])), ("green", flat([20, 200, 40, 255])), ("blue", flat([20, 40, 220, 255]))];
    for (name, img) in &shots {
        index.insert(Key::of(name), &model.encode_image(img).unwrap()).unwrap();
    }
    let hits = index.search(&model.encode_text("a green image").unwrap(), 3).unwrap();
    assert_eq!(hits[0].key, Key::of("green"));
    let _ = std::fs::remove_dir_all(&dir);
}
