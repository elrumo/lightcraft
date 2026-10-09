//! The words in photos: what OCR read, kept per photo and searched as text. Derived data like the
//! embedding index (filed by content hash, recreated by reading the photos again, never part of the
//! catalog), but of variable length, so it has its own small append-only file.
//!
//! Matching is forgiving on purpose, because OCR is: case, accents and full-width forms are folded
//! away, hiragana and katakana are the same, a single wrong letter in a long word still matches,
//! and Chinese/Japanese (which have no spaces between words) match as phrases or, failing that,
//! when most of their character pairs are there.
// ponytail: a linear scan over every photo's text (milliseconds for 100k photos with text on a quarter
// of them); an inverted index would be the next step if that stops being true.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::Path;

use unicode_normalization::UnicodeNormalization;

use crate::Error;
use crate::store::{KEY_LEN, Key};

const MAGIC: [u8; 8] = *b"LCVTEXT1";
const HEADER_LEN: usize = 32;
/// Longest engine id, in bytes.
pub const ENGINE_LEN: usize = 16;
/// Most text kept for one photo, in bytes.
pub const MAX_TEXT: usize = 64 << 10;
/// Files larger than this are refused.
const MAX_FILE: u64 = 2 << 30;
/// A query is cut to this many characters and this many words.
const MAX_QUERY: usize = 512;
const MAX_TERMS: usize = 12;

/// One photo found by text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextHit {
    pub key: Key,
    /// 0 to 1.2 (1 = every word found exactly; fuzzy and partial matches score less).
    pub score: f32,
}

/// A text match scores [`TEXT_SCORE_BASE`] plus its own score: above any cosine similarity (at
/// most 1), so searching words and looks together puts the photos with the words first.
pub const TEXT_SCORE_BASE: f32 = 2.0;

/// The results of searching a query's words and its look together: the photos with the words
/// first (scores from [`TEXT_SCORE_BASE`]), then the look-alikes they don't already include, at
/// most `limit` in all.
pub fn merge(text: &[TextHit], looks: Vec<crate::Hit>, limit: usize) -> Vec<crate::Hit> {
    let mut out: Vec<crate::Hit> = text.iter().map(|h| crate::Hit { key: h.key, score: TEXT_SCORE_BASE + h.score }).collect();
    let seen: std::collections::HashSet<Key> = out.iter().map(|h| h.key).collect();
    out.extend(looks.into_iter().filter(|h| !seen.contains(&h.key)));
    out.truncate(limit);
    out
}

/// What [`TextIndex::import`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    pub added: usize,
    pub skipped: usize,
}

/// `s` as it is compared: compatibility-normalised (full-width and half-width forms, ligatures),
/// lower-cased, without accents (combining marks U+0300–U+036F; the Japanese voiced marks are not
/// among them), hiragana turned into katakana.
pub fn normalize(s: &str) -> String {
    s.nfkc().flat_map(char::to_lowercase).nfd().filter(|c| !('\u{300}'..='\u{36f}').contains(c)).nfc().map(fold_kana).collect()
}

fn fold_kana(c: char) -> char {
    match c {
        '\u{3041}'..='\u{3096}' | '\u{309d}' | '\u{309e}' => char::from_u32(c as u32 + 0x60).unwrap_or(c),
        _ => c,
    }
}

/// Han, kana, hangul: scripts written without spaces between words.
fn is_cjk(c: char) -> bool {
    matches!(c, '\u{2e80}'..='\u{9fff}' | '\u{ac00}'..='\u{d7af}' | '\u{f900}'..='\u{faff}' | '\u{ff66}'..='\u{ff9f}' | '\u{20000}'..='\u{2fa1f}')
}

fn bigrams(s: &str) -> Vec<(char, char)> {
    let c: Vec<char> = s.chars().collect();
    c.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?))).collect()
}

/// `a` and `b` differ by at most one inserted, deleted or substituted character.
fn within_one_edit(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let (mut i, mut j, mut edits) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        if a.get(i) == b.get(j) {
            i += 1;
            j += 1;
            continue;
        }
        edits += 1;
        if edits > 1 {
            return false;
        }
        match a.len().cmp(&b.len()) {
            std::cmp::Ordering::Greater => i += 1,
            std::cmp::Ordering::Less => j += 1,
            std::cmp::Ordering::Equal => {
                i += 1;
                j += 1;
            }
        }
    }
    edits + (a.len() - i) + (b.len() - j) <= 1
}

struct Doc {
    key: Key,
    text: String,
    norm: String,
}

/// How well one query word matches a photo's normalised text (0 = not at all).
fn term_score(term: &str, norm: &str) -> f32 {
    if norm.contains(term) {
        return 1.0;
    }
    let n = term.chars().count();
    if term.chars().any(is_cjk) {
        // most of the character pairs are there (OCR drops or swaps the odd character)
        let pairs = bigrams(term);
        if n >= 3 && !pairs.is_empty() {
            let have = bigrams(norm);
            let hit = pairs.iter().filter(|p| have.contains(p)).count();
            let cover = hit as f32 / pairs.len() as f32;
            if cover >= 0.75 {
                return 0.9 * cover;
            }
        }
        return 0.0;
    }
    // one wrong letter in a long word
    if n >= 5 && norm.split(|c: char| !c.is_alphanumeric()).any(|w| w.chars().count().abs_diff(n) <= 1 && within_one_edit(term, w)) {
        return 0.8;
    }
    0.0
}

pub struct TextIndex {
    file: Option<File>,
    engine: String,
    docs: Vec<Doc>,
    pos: HashMap<Key, usize>,
    /// Bytes in the file (where a failed append rolls back to).
    len: u64,
}

fn header(engine: &str) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[..8].copy_from_slice(&MAGIC);
    let e = engine.as_bytes();
    h[16..16 + e.len().min(ENGINE_LEN)].copy_from_slice(e.get(..ENGINE_LEN).unwrap_or(e));
    h
}

fn check_header(h: &[u8], engine: &str) -> Result<(), Error> {
    if h.get(..8) != Some(&MAGIC[..]) {
        return Err(Error::Format("bad magic"));
    }
    let found = h.get(16..HEADER_LEN).unwrap_or(&[]).split(|&b| b == 0).next().unwrap_or(&[]);
    if found != engine.as_bytes() {
        return Err(Error::Mismatch { found: String::from_utf8_lossy(found).into_owned(), wanted: engine.to_string() });
    }
    Ok(())
}

fn check_engine(engine: &str) -> Result<(), Error> {
    if engine.is_empty() || engine.len() > ENGINE_LEN || !engine.is_ascii() || engine.bytes().any(|b| b == 0) {
        return Err(Error::Invalid("engine id"));
    }
    Ok(())
}

/// A record: key, length, text, checksum of the three.
fn record(key: &Key, text: &str) -> Vec<u8> {
    let mut r = Vec::with_capacity(KEY_LEN + 8 + text.len());
    r.extend_from_slice(&key.0);
    r.extend_from_slice(&(text.len() as u32).to_le_bytes());
    r.extend_from_slice(text.as_bytes());
    let crc = crc32fast::hash(&r);
    r.extend_from_slice(&crc.to_le_bytes());
    r
}

/// Reads one record; `Ok(None)` at a clean end, `Err` at anything else (cut short, over the size
/// limit, wrong checksum, not UTF-8).
fn read_record(r: &mut impl Read) -> Result<Option<(Key, String)>, ()> {
    let mut head = [0u8; KEY_LEN + 4];
    let mut got = 0;
    while got < head.len() {
        match r.read(head.get_mut(got..).ok_or(())?) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(()),
            Ok(n) => got += n,
            Err(_) => return Err(()),
        }
    }
    let key = Key::read(&head).ok_or(())?;
    let len = u32::from_le_bytes(head.get(KEY_LEN..).and_then(|b| <[u8; 4]>::try_from(b).ok()).ok_or(())?) as usize;
    if len > MAX_TEXT {
        return Err(());
    }
    let mut body = vec![0u8; len + 4];
    r.read_exact(&mut body).map_err(|_| ())?;
    let (text, crc) = body.split_at(len);
    let mut check = head.to_vec();
    check.extend_from_slice(text);
    if crc32fast::hash(&check).to_le_bytes() != crc {
        return Err(());
    }
    Ok(Some((key, String::from_utf8(text.to_vec()).map_err(|_| ())?)))
}

/// `text` as stored: whitespace-trimmed and cut to [`MAX_TEXT`] at a character boundary.
fn clean(text: &str) -> &str {
    let t = text.trim();
    let mut end = t.len().min(MAX_TEXT);
    while !t.is_char_boundary(end) {
        end -= 1;
    }
    t.get(..end).unwrap_or("")
}

impl TextIndex {
    /// Opens (or creates) the index at `path` for text read by `engine` (an id such as
    /// `ppocr6-small`). A file from another engine is [`Error::Mismatch`]; a damaged tail is cut
    /// off and the records before it are kept.
    pub fn open(path: &Path, engine: &str) -> Result<TextIndex, Error> {
        check_engine(engine)?;
        let mut file = OpenOptions::new().read(true).append(true).create(true).open(path)?;
        let size = file.metadata()?.len();
        if size > MAX_FILE {
            return Err(Error::Format("file too large"));
        }
        let mut ix = TextIndex { file: None, engine: engine.to_string(), docs: Vec::new(), pos: HashMap::new(), len: HEADER_LEN as u64 };
        if size < HEADER_LEN as u64 {
            file.set_len(0)?;
            file.write_all(&header(engine))?;
            ix.file = Some(file);
            return Ok(ix);
        }
        let mut reader = BufReader::new(&file);
        let mut h = [0u8; HEADER_LEN];
        reader.read_exact(&mut h)?;
        check_header(&h, engine)?;
        let mut good = HEADER_LEN as u64;
        loop {
            match read_record(&mut reader) {
                Ok(Some((key, text))) => {
                    good += (KEY_LEN + 8 + text.len()) as u64;
                    ix.add(key, text);
                }
                Ok(None) => break,
                Err(()) => break,
            }
        }
        drop(reader);
        if good != size {
            file.set_len(good)?;
        }
        ix.len = good;
        ix.file = Some(file);
        Ok(ix)
    }

    /// An index that lives in memory only.
    pub fn in_memory(engine: &str) -> Result<TextIndex, Error> {
        check_engine(engine)?;
        Ok(TextIndex { file: None, engine: engine.to_string(), docs: Vec::new(), pos: HashMap::new(), len: HEADER_LEN as u64 })
    }

    fn add(&mut self, key: Key, text: String) -> bool {
        if self.pos.contains_key(&key) {
            return false;
        }
        let norm = normalize(&text);
        self.pos.insert(key, self.docs.len());
        self.docs.push(Doc { key, text, norm });
        true
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    pub fn engine(&self) -> &str {
        &self.engine
    }

    pub fn contains(&self, key: &Key) -> bool {
        self.pos.contains_key(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &Key> {
        self.docs.iter().map(|d| &d.key)
    }

    /// The text kept for a photo (`Some("")`: it was read and has none).
    pub fn text(&self, key: &Key) -> Option<&str> {
        self.pos.get(key).and_then(|&i| self.docs.get(i)).map(|d| d.text.as_str())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if let Some(f) = &mut self.file {
            if let Err(e) = f.write_all(bytes) {
                let _ = f.set_len(self.len);
                return Err(e.into());
            }
            self.len += bytes.len() as u64;
        }
        Ok(())
    }

    /// Keeps the text read from a photo (empty for "no text": the photo isn't read again).
    /// `Ok(false)` when it already has some.
    pub fn insert(&mut self, key: Key, text: &str) -> Result<bool, Error> {
        if self.pos.contains_key(&key) {
            return Ok(false);
        }
        let text = clean(text).to_string();
        self.append(&record(&key, &text))?;
        self.add(key, text);
        Ok(true)
    }

    /// The photos whose text matches `query`, best first (all of its words must be found).
    pub fn search(&self, query: &str, limit: usize) -> Vec<TextHit> {
        let q = normalize(&query.chars().take(MAX_QUERY).collect::<String>());
        let terms: Vec<&str> = q.split_whitespace().take(MAX_TERMS).collect();
        if terms.is_empty() || limit == 0 {
            return Vec::new();
        }
        let phrase = terms.len() > 1;
        let joined = terms.join(" ");
        let mut hits: Vec<(f32, usize)> = Vec::new();
        for (i, d) in self.docs.iter().enumerate() {
            if d.norm.is_empty() {
                continue;
            }
            let mut sum = 0.0;
            let mut all = true;
            for t in &terms {
                let s = term_score(t, &d.norm);
                if s <= 0.0 {
                    all = false;
                    break;
                }
                sum += s;
            }
            if all {
                let bonus = if phrase && d.norm.contains(&joined) { 0.2 } else { 0.0 };
                hits.push((sum / terms.len() as f32 + bonus, i));
            }
        }
        hits.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        hits.truncate(limit);
        hits.into_iter().filter_map(|(score, i)| self.docs.get(i).map(|d| TextHit { key: d.key, score })).collect()
    }

    /// A file's worth of records (header first) for the photos `skip` doesn't name, at most `max`:
    /// what a device sends a server that lacks them. [`TextIndex::import`] reads it back.
    pub fn export(&self, skip: impl Fn(&Key) -> bool, max: usize) -> Vec<u8> {
        let mut out = header(&self.engine).to_vec();
        for d in self.docs.iter().filter(|d| !skip(&d.key)).take(max) {
            out.extend_from_slice(&record(&d.key, &d.text));
        }
        out
    }

    /// Adds the records of an [`TextIndex::export`] for the photos `allow` accepts, skipping those
    /// already here. All or nothing: another engine, a cut-off or damaged record rejects the lot.
    pub fn import(&mut self, bytes: &[u8], allow: impl Fn(&Key) -> bool) -> Result<Imported, Error> {
        check_header(bytes.get(..HEADER_LEN).ok_or(Error::Format("short header"))?, &self.engine)?;
        let mut body = bytes.get(HEADER_LEN..).unwrap_or(&[]);
        let mut fresh: Vec<(Key, String)> = Vec::new();
        let mut out = Imported::default();
        let mut seen = std::collections::HashSet::new();
        loop {
            match read_record(&mut body) {
                Ok(Some((key, text))) => {
                    if self.pos.contains_key(&key) || !allow(&key) || !seen.insert(key) {
                        out.skipped += 1;
                    } else {
                        fresh.push((key, clean(&text).to_string()));
                    }
                }
                Ok(None) => break,
                Err(()) => return Err(Error::Format("damaged or truncated record")),
            }
        }
        let mut recs = Vec::new();
        for (k, t) in &fresh {
            recs.extend_from_slice(&record(k, t));
        }
        if !recs.is_empty() {
            self.append(&recs)?;
        }
        for (k, t) in fresh {
            self.add(k, t);
            out.added += 1;
        }
        Ok(out)
    }
}
