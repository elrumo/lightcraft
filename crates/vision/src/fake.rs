//! A deterministic stand-in for the model, for tests: a photo's vector is its average colour
//! (centred on grey), a sentence's vector is the colour its words name ("red", "blue", "dark"…).
//! So "a red sunset" finds the reddest photos, with no weights and no GPU.

use lightcraft_raster::Rgba8;

use crate::{Embedder, Error};

pub const FAKE_ID: &str = "fake-colour";
pub const FAKE_DIM: usize = 4;

/// Every vector keeps a small constant last component so none is all zero (a grey photo, a
/// sentence naming no colour).
const BIAS: f32 = 0.05;

#[derive(Clone, Copy, Debug, Default)]
pub struct FakeEmbedder;

fn colour_of(word: &str) -> Option<[f32; 3]> {
    Some(match word {
        "red" => [0.5, -0.5, -0.5],
        "green" => [-0.5, 0.5, -0.5],
        "blue" => [-0.5, -0.5, 0.5],
        "yellow" => [0.5, 0.5, -0.5],
        "white" | "bright" => [0.5, 0.5, 0.5],
        "black" | "dark" => [-0.5, -0.5, -0.5],
        _ => return None,
    })
}

impl Embedder for FakeEmbedder {
    fn model_id(&self) -> &str {
        FAKE_ID
    }

    fn dim(&self) -> usize {
        FAKE_DIM
    }

    fn encode_text(&self, text: &str) -> Result<Vec<f32>, Error> {
        let mut v = [0f32; 3];
        for c in text.to_lowercase().split(|c: char| !c.is_alphabetic()).filter_map(colour_of) {
            v.iter_mut().zip(c).for_each(|(a, b)| *a += b);
        }
        Ok(vec![v[0], v[1], v[2], BIAS])
    }

    fn encode_images(&self, imgs: &[&Rgba8]) -> Result<Vec<Vec<f32>>, Error> {
        imgs.iter()
            .map(|img| {
                if img.data.is_empty() {
                    return Err(Error::Invalid("image size"));
                }
                let mut sum = [0f64; 3];
                for p in &img.data {
                    for (s, &c) in sum.iter_mut().zip(p) {
                        *s += f64::from(c);
                    }
                }
                let n = img.data.len() as f64 * 255.0;
                Ok(vec![(sum[0] / n - 0.5) as f32, (sum[1] / n - 0.5) as f32, (sum[2] / n - 0.5) as f32, BIAS])
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EmbeddingIndex, Key};
    use lightcraft_raster::Image;

    fn flat(px: [u8; 4]) -> Rgba8 {
        Image { width: 4, height: 4, data: vec![px; 16] }
    }

    #[test]
    fn a_colour_word_finds_the_photo_of_that_colour() {
        let m = FakeEmbedder;
        let mut ix = EmbeddingIndex::in_memory(m.model_id(), m.dim()).unwrap();
        let shots = [("r", [220, 20, 20, 255]), ("g", [20, 200, 40, 255]), ("b", [20, 40, 220, 255]), ("grey", [128, 128, 128, 255])];
        for (name, px) in shots {
            ix.insert(Key::of(name), &m.encode_images(&[&flat(px)]).unwrap().remove(0)).unwrap();
        }
        for (word, want) in [("a Red sunset", "r"), ("green hills", "g"), ("the BLUE sea", "b")] {
            let hits = ix.search(&m.encode_text(word).unwrap(), 4).unwrap();
            assert_eq!(hits[0].key, Key::of(want), "{word}");
        }
        // a sentence naming no colour still has a vector
        assert_eq!(m.encode_text("a quiet street").unwrap().len(), FAKE_DIM);
    }
}
