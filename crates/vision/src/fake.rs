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

pub const FAKE_READER_ID: &str = "fake-reader";

/// A stand-in for the text reader: a photo "says" what its dominant colour suggests (a red photo
/// has a stop sign in it, a blue one a harbour hotel, a green one a café; grey ones have no text).
#[derive(Clone, Copy, Debug, Default)]
pub struct FakeReader;

impl crate::TextReader for FakeReader {
    fn engine(&self) -> &str {
        FAKE_READER_ID
    }

    fn text(&self, img: &Rgba8) -> Result<String, Error> {
        if img.data.is_empty() {
            return Err(Error::Invalid("image size"));
        }
        let mut sum = [0u64; 3];
        for p in &img.data {
            for (s, &c) in sum.iter_mut().zip(p) {
                *s += u64::from(c);
            }
        }
        let [r, g, b] = sum;
        let margin = img.data.len() as u64 * 30;
        Ok(if r > g + margin && r > b + margin {
            "STOP\nOpen 24 hours"
        } else if b > r + margin && b > g + margin {
            "Harbour Hotel\nRoom 12"
        } else if g > r + margin && g > b + margin {
            "Café Verde"
        } else {
            ""
        }
        .to_string())
    }
}

pub const FAKE_FACES_ID: &str = "fake-faces";

/// A stand-in for the face finder: the left and right halves of a photo each "have a face" when
/// they are bright enough, and a face "looks like" its half's dominant colour (red, green and blue
/// halves are three people, with a little variation, so they cluster). Rects are fixed boxes.
#[derive(Clone, Copy, Debug, Default)]
pub struct FakeFaces;

impl crate::faces::FaceEngine for FakeFaces {
    fn engine(&self) -> &str {
        FAKE_FACES_ID
    }

    fn faces(&self, img: &Rgba8) -> Result<Vec<crate::faces::FaceFound>, Error> {
        if img.data.is_empty() || img.width == 0 {
            return Err(Error::Invalid("image size"));
        }
        let mut out = Vec::new();
        for (half, rect) in [(0usize, [0.1f32, 0.2, 0.45, 0.8]), (1, [0.55, 0.2, 0.9, 0.8])] {
            let (mut sum, mut n) = ([0f64; 3], 0f64);
            for row in img.data.chunks(img.width) {
                let (left, right) = row.split_at(row.len() / 2);
                for p in if half == 0 { left } else { right } {
                    for (s, &c) in sum.iter_mut().zip(p) {
                        *s += f64::from(c);
                    }
                    n += 1.0;
                }
            }
            if n == 0.0 {
                continue;
            }
            let mean = sum.map(|s| (s / n) as f32);
            let top = mean.iter().copied().fold(0f32, f32::max);
            if top < 100.0 {
                continue;
            }
            let dominant = mean.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(i, _)| i);
            let mut e = vec![0f32; crate::faces::DIM];
            if let Some(slot) = e.get_mut(dominant) {
                *slot = 1.0;
            }
            for (i, m) in mean.iter().enumerate() {
                if let Some(slot) = e.get_mut(8 + i) {
                    *slot = m / 2550.0;
                }
            }
            out.push(crate::faces::FaceFound { rect, score: 0.95, embedding: e });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EmbeddingIndex, Key};
    use lightcraft_raster::Image;

    #[test]
    fn the_fake_reader_reads_a_sign_off_a_coloured_photo() {
        use crate::TextReader;
        let r = FakeReader;
        assert!(r.text(&flat([220, 20, 20, 255])).unwrap().contains("STOP"));
        assert!(r.text(&flat([20, 40, 220, 255])).unwrap().contains("Harbour"));
        assert!(r.text(&flat([128, 128, 128, 255])).unwrap().is_empty());
        assert!(r.text(&Image { width: 0, height: 0, data: vec![] }).is_err());
    }

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
