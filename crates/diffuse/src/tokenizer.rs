//! CLIP's tokenizer: text to the 77 token ids Stable Diffusion's text encoders read.
//!
//! Byte-level BPE as OpenAI's CLIP does it: the text cleaned (whitespace runs to one space,
//! lower-cased), split into words, numbers and other symbols, each piece's bytes mapped to
//! printable characters, the last symbol of a word marked `</w>`, and the merges applied in rank
//! order. The vocabulary (`vocab.json`) and merges (`merges.txt`) come with each model.

use std::collections::HashMap;
use std::path::Path;

use crate::{Error, Result};

/// Tokens per prompt (start, up to 75 tokens, end, then padding).
pub const LENGTH: usize = 77;
/// CLIP uses the first 48 894 merges (`49152 - 256 - 2`).
const MERGES: usize = 49152 - 256 - 2;
const BOS: &str = "<|startoftext|>";
const EOS: &str = "<|endoftext|>";
/// Largest vocabulary or merges file read.
const MAX_FILE: u64 = 16 << 20;

/// What fills a prompt after its end token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pad {
    /// The end token again (Stable Diffusion 1.5, SDXL's first encoder).
    Eos,
    /// `!`, id 0 (SDXL's second encoder).
    Bang,
}

pub struct Tokenizer {
    vocab: HashMap<String, u32>,
    ranks: HashMap<(String, String), usize>,
    bytes: [char; 256],
    bos: u32,
    eos: u32,
    pad: u32,
}

/// GPT-2's map from bytes to printable characters.
fn byte_chars() -> [char; 256] {
    let mut out = ['\0'; 256];
    let printable = |b: u32| (33..=126).contains(&b) || (161..=172).contains(&b) || (174..=255).contains(&b);
    let mut n = 0u32;
    for (b, slot) in (0u32..256).zip(out.iter_mut()) {
        let c = if printable(b) {
            b
        } else {
            n += 1;
            255 + n
        };
        *slot = char::from_u32(c).unwrap_or('?');
    }
    out
}

fn read(path: &Path) -> Result<String> {
    let len = std::fs::metadata(path).map_err(|_| Error::Model(format!("{} is missing", path.display())))?.len();
    if len > MAX_FILE {
        return Err(Error::Model(format!("{} is too large to be a tokenizer file", path.display())));
    }
    std::fs::read_to_string(path).map_err(|e| Error::Model(format!("can't read {}: {e}", path.display())))
}

/// What a character is, for splitting the text into pieces.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Letter,
    Digit,
    Space,
    Other,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphabetic() {
        Class::Letter
    } else if c.is_numeric() {
        Class::Digit
    } else {
        Class::Other
    }
}

/// The pieces of cleaned, lower-cased `text`, as CLIP's pattern splits it: the special tokens,
/// the contractions (`'s`, `'t`, `'re`, `'ve`, `'m`, `'ll`, `'d`), runs of letters, single digits,
/// and runs of other symbols.
fn pieces(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let rest: String = chars.get(i..).map(|r| r.iter().take(16).collect()).unwrap_or_default();
        if let Some(sp) = [BOS, EOS].into_iter().find(|s| rest.starts_with(s)) {
            out.push(sp.to_string());
            i += sp.chars().count();
            continue;
        }
        if c == '\''
            && let Some(k) = ["'s", "'t", "'re", "'ve", "'m", "'ll", "'d"].into_iter().find(|k| rest.starts_with(k))
        {
            out.push(k.to_string());
            i += k.chars().count();
            continue;
        }
        match class(c) {
            Class::Space => i += 1,
            Class::Digit => {
                out.push(c.to_string());
                i += 1;
            }
            k => {
                let start = i;
                while chars.get(i).is_some_and(|&d| class(d) == k) {
                    i += 1;
                }
                out.push(chars.get(start..i).map(|s| s.iter().collect()).unwrap_or_default());
            }
        }
    }
    out
}

impl Tokenizer {
    /// From a model's `vocab.json` and `merges.txt`.
    pub fn load(vocab: &Path, merges: &Path, pad: Pad) -> Result<Tokenizer> {
        let v: HashMap<String, u32> =
            serde_json::from_str(&read(vocab)?).map_err(|e| Error::Model(format!("{} isn't a CLIP vocabulary: {e}", vocab.display())))?;
        let m = read(merges)?;
        let ranks: HashMap<(String, String), usize> = m
            .lines()
            .skip(1)
            .take(MERGES)
            .filter_map(|l| l.split_once(' ').map(|(a, b)| (a.to_string(), b.to_string())))
            .enumerate()
            .map(|(r, p)| (p, r))
            .collect();
        Tokenizer::new(v, ranks, pad)
    }

    fn new(vocab: HashMap<String, u32>, ranks: HashMap<(String, String), usize>, pad: Pad) -> Result<Tokenizer> {
        let id = |s: &str| vocab.get(s).copied().ok_or_else(|| Error::Model(format!("the CLIP vocabulary has no {s}")));
        let (bos, eos) = (id(BOS)?, id(EOS)?);
        let pad = match pad {
            Pad::Eos => eos,
            Pad::Bang => id("!")?,
        };
        Ok(Tokenizer { vocab, ranks, bytes: byte_chars(), bos, eos, pad })
    }

    /// The BPE symbols of one piece (already byte-mapped).
    fn bpe(&self, piece: &str) -> Vec<String> {
        let mut word: Vec<String> = piece.chars().map(String::from).collect();
        if let Some(last) = word.last_mut() {
            last.push_str("</w>");
        }
        while word.len() > 1 {
            let best = word
                .windows(2)
                .enumerate()
                .filter_map(|(i, w)| match w {
                    [a, b] => self.ranks.get(&(a.clone(), b.clone())).map(|r| (*r, i)),
                    _ => None,
                })
                .min();
            let Some((_, i)) = best else { break };
            let (Some(a), Some(b)) = (word.get(i).cloned(), word.get(i + 1).cloned()) else { break };
            // merge every occurrence of the pair, left to right
            let mut next = Vec::with_capacity(word.len());
            let mut i = 0;
            while i < word.len() {
                match (word.get(i), word.get(i + 1)) {
                    (Some(x), Some(y)) if *x == a && *y == b => {
                        next.push(format!("{x}{y}"));
                        i += 2;
                    }
                    (Some(x), _) => {
                        next.push(x.clone());
                        i += 1;
                    }
                    _ => break,
                }
            }
            word = next;
        }
        word
    }

    /// The end token's id.
    pub fn eos(&self) -> u32 {
        self.eos
    }

    /// The ids of `text` (start token, the text's tokens, end token), without padding.
    pub fn ids(&self, text: &str) -> Vec<u32> {
        let clean = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        let mut out = vec![self.bos];
        for p in pieces(&clean) {
            if p == BOS || p == EOS {
                out.push(if p == BOS { self.bos } else { self.eos });
                continue;
            }
            let mapped: String = p.bytes().map(|b| self.bytes.get(b as usize).copied().unwrap_or('?')).collect();
            for sym in self.bpe(&mapped) {
                if let Some(&id) = self.vocab.get(&sym) {
                    out.push(id);
                }
            }
        }
        out.push(self.eos);
        out
    }

    /// The [`LENGTH`] ids the text encoder reads: truncated to fit (keeping the end token), padded.
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut ids = self.ids(text);
        if ids.len() > LENGTH {
            ids.truncate(LENGTH - 1);
            ids.push(self.eos);
        }
        ids.resize(LENGTH, self.pad);
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Tokenizer {
        let vocab: HashMap<String, u32> = [
            ("<|startoftext|>", 100),
            ("<|endoftext|>", 101),
            ("!", 0),
            ("a</w>", 1),
            ("c", 2),
            ("at</w>", 3),
            ("cat</w>", 4),
            ("1</w>", 5),
            (",</w>", 6),
            ("'s</w>", 7),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let ranks = [(("a", "t</w>"), 0), (("c", "at</w>"), 1)].into_iter().map(|((a, b), r)| ((a.to_string(), b.to_string()), r)).collect();
        Tokenizer::new(vocab, ranks, Pad::Bang).unwrap()
    }

    #[test]
    fn splits_like_clips_pattern() {
        assert_eq!(pieces("a cat's 12 cats, ok!?"), ["a", "cat", "'s", "1", "2", "cats", ",", "ok", "!?"]);
        assert_eq!(pieces("<|startoftext|>hi"), ["<|startoftext|>", "hi"]);
        assert!(pieces("").is_empty());
        assert_eq!(pieces("  \u{e9}t\u{e9}  "), ["\u{e9}t\u{e9}"]);
    }

    #[test]
    fn merges_by_rank_and_pads() {
        let t = tiny();
        assert_eq!(t.ids("A   CAT"), vec![100, 1, 4, 101]);
        let e = t.encode("a cat, 1");
        assert_eq!(e.len(), LENGTH);
        assert_eq!(&e[..6], &[100, 1, 4, 6, 5, 101]);
        assert!(e[6..].iter().all(|&i| i == 0));
        // a long prompt keeps its end token
        let long = t.encode(&"a ".repeat(200));
        assert_eq!((long.len(), long[0], long[LENGTH - 1]), (LENGTH, 100, 101));
    }

    #[test]
    fn byte_map_is_one_to_one() {
        let m = byte_chars();
        let mut seen: Vec<char> = m.to_vec();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 256);
        assert_eq!((m[b'a' as usize], m[b' ' as usize]), ('a', '\u{120}'));
    }

    /// The real vocabulary, when a model folder is given (`$LIGHTCRAFT_SD15`).
    #[test]
    fn real_vocabulary() {
        let Some(dir) = std::env::var_os("LIGHTCRAFT_SD15").map(std::path::PathBuf::from) else { return };
        let t = Tokenizer::load(&dir.join("tokenizer/vocab.json"), &dir.join("tokenizer/merges.txt"), Pad::Eos).unwrap();
        assert_eq!(t.ids("a photo of a cat"), vec![49406, 320, 1125, 539, 320, 2368, 49407]);
    }
}
