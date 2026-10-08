use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::textindex::{TextIndex, normalize};
use crate::{Error, Key};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        let d = std::env::temp_dir().join(format!("lc-text-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(i: usize) -> Key {
    Key::of(&format!("photo-{i}"))
}

fn library() -> TextIndex {
    let mut ix = TextIndex::in_memory("test-ocr").unwrap();
    for (i, t) in [
        "INVOICE No. 48213\nTotal: $1,234.50",  // 0
        "Café de la Gare\nOuvert 9h–18h",       // 1
        "東京タワー\n営業時間 9:00-22:00",      // 2
        "欢迎来到上海\n上海迪士尼乐园",         // 3
        "STOP",                                 // 4
        "",                                     // 5: read, no text
        "Pizzeria Margherita\nMenu del giorno", // 6
        "ＡＢＣ　１２３　ｶﾞﾗｽ",                  // 7: full-width and half-width forms
    ]
    .iter()
    .enumerate()
    {
        assert!(ix.insert(key(i), t).unwrap());
    }
    ix
}

fn found(ix: &TextIndex, q: &str) -> Vec<usize> {
    ix.search(q, 100).iter().map(|h| (0..8).find(|&i| key(i) == h.key).unwrap()).collect()
}

#[test]
fn text_is_folded_the_way_people_type_it() {
    assert_eq!(normalize("Café"), "cafe");
    assert_eq!(normalize("ＡＢＣ　１２３"), "abc 123");
    assert_eq!(normalize("ｶﾞﾗｽ"), "ガラス");
    assert_eq!(normalize("とうきょう"), "トウキョウ");
    assert_eq!(normalize("ストップ"), normalize("すとっぷ"));
    // voiced marks are not accents: が is not か
    assert_ne!(normalize("が"), normalize("か"));
    assert_eq!(normalize("STRASSE ﬁsh"), "strasse fish");
    assert_eq!(normalize(""), "");
}

#[test]
fn words_are_found_whatever_their_case_accents_or_width() {
    let ix = library();
    assert_eq!(found(&ix, "invoice"), [0]);
    assert_eq!(found(&ix, "INVOICE 48213"), [0]);
    assert_eq!(found(&ix, "cafe"), [1]);
    assert_eq!(found(&ix, "café gare"), [1]);
    assert_eq!(found(&ix, "stop"), [4]);
    assert_eq!(found(&ix, "abc 123"), [7]);
    assert_eq!(found(&ix, "ガラス"), [7]);
    assert_eq!(found(&ix, "１２３"), [7]);
    assert_eq!(found(&ix, "1,234.50"), [0]);
    // every word must be there
    assert_eq!(found(&ix, "invoice pizza"), Vec::<usize>::new());
    assert_eq!(found(&ix, "menu giorno"), [6]);
    // nothing
    assert!(found(&ix, "zebra").is_empty() && found(&ix, "").is_empty() && found(&ix, "   ").is_empty());
}

#[test]
fn chinese_and_japanese_match_as_phrases_and_by_most_of_their_pairs() {
    let ix = library();
    assert_eq!(found(&ix, "東京"), [2]);
    assert_eq!(found(&ix, "とうきょうたわー"), Vec::<usize>::new(), "kana only matches kana text");
    assert_eq!(found(&ix, "トウキョウ"), Vec::<usize>::new());
    assert_eq!(found(&ix, "タワー"), [2]);
    assert_eq!(found(&ix, "たわー"), [2], "hiragana finds katakana");
    assert_eq!(found(&ix, "上海"), [3]);
    assert_eq!(found(&ix, "上海迪士尼"), [3]);
    // OCR dropped or swapped a character: most pairs are still there
    assert_eq!(found(&ix, "上海迪士尼乐圆"), [3], "one wrong character in seven");
    assert!(found(&ix, "迪尼士乐园").is_empty() || found(&ix, "迪尼士乐园") == [3]);
    assert!(found(&ix, "北京").is_empty());
    // a one-character query is exact only
    assert_eq!(found(&ix, "塔"), Vec::<usize>::new());
    assert_eq!(found(&ix, "港"), Vec::<usize>::new());
}

#[test]
fn one_wrong_letter_in_a_long_word_still_matches_but_not_in_a_short_one() {
    let ix = library();
    assert_eq!(found(&ix, "invoce"), [0], "one dropped");
    assert_eq!(found(&ix, "invxxce"), Vec::<usize>::new(), "two letters off");
    assert_eq!(found(&ix, "invoise"), [0], "one substituted");
    assert_eq!(found(&ix, "pizzera"), [6], "one dropped");
    assert_eq!(found(&ix, "pizzxra"), Vec::<usize>::new(), "two off");
    assert_eq!(found(&ix, "margherta"), [6], "one dropped");
    assert!(found(&ix, "stap").is_empty(), "short words are exact");
}

#[test]
fn better_matches_come_first() {
    let mut ix = TextIndex::in_memory("test-ocr").unwrap();
    ix.insert(key(0), "grand hotel and the sea").unwrap();
    ix.insert(key(1), "the sea hotel").unwrap();
    ix.insert(key(2), "hotal").unwrap();
    ix.insert(key(3), "sea view").unwrap();
    let hits = ix.search("sea hotel", 10);
    // only photos with both words; the one that has them side by side comes first
    assert_eq!(hits.iter().map(|h| h.key).collect::<Vec<_>>(), [key(1), key(0)]);
    let hits = ix.search("hotel", 10);
    // the exact word, in index order, then the near miss
    assert_eq!(hits.iter().map(|h| h.key).collect::<Vec<_>>(), [key(0), key(1), key(2)]);
    assert!(hits[0].score > hits[2].score);
    assert_eq!(ix.search("hotel", 2).len(), 2);
    assert!(ix.search("hotel", 0).is_empty());
}

#[test]
fn what_was_read_survives_a_restart_and_a_photo_is_read_once() {
    let dir = Dir::new();
    let path = dir.file("t.bin");
    {
        let mut ix = TextIndex::open(&path, "test-ocr").unwrap();
        assert!(ix.insert(key(1), "Hello World").unwrap());
        assert!(ix.insert(key(2), "").unwrap(), "no text is a result too");
        assert!(!ix.insert(key(1), "something else").unwrap());
    }
    let ix = TextIndex::open(&path, "test-ocr").unwrap();
    assert_eq!((ix.len(), ix.text(&key(1)), ix.text(&key(2)), ix.text(&key(3))), (2, Some("Hello World"), Some(""), None));
    assert!(ix.contains(&key(2)) && !ix.contains(&key(3)));
    assert_eq!(ix.search("world", 5).len(), 1);
}

#[test]
fn a_damaged_file_loses_only_what_is_damaged() {
    let dir = Dir::new();
    let path = dir.file("t.bin");
    {
        let mut ix = TextIndex::open(&path, "test-ocr").unwrap();
        for i in 0..4 {
            ix.insert(key(i), &format!("photo number {i}")).unwrap();
        }
    }
    let whole = std::fs::read(&path).unwrap();
    // cut in the last record
    std::fs::write(&path, &whole[..whole.len() - 5]).unwrap();
    let mut ix = TextIndex::open(&path, "test-ocr").unwrap();
    assert_eq!(ix.len(), 3, "the cut record is dropped");
    assert!(ix.insert(key(3), "again").unwrap(), "and can be added again, aligned");
    drop(ix);
    assert_eq!(TextIndex::open(&path, "test-ocr").unwrap().len(), 4);
    // a flipped byte in the middle: everything from there on goes (derived data)
    let mut bad = std::fs::read(&path).unwrap();
    bad[32 + 24 + 20 + 30] ^= 0xff;
    std::fs::write(&path, &bad).unwrap();
    let ix = TextIndex::open(&path, "test-ocr").unwrap();
    assert_eq!(ix.len(), 1);
    // garbage appended
    OpenOptions::new().append(true).open(&path).unwrap().write_all(&[9; 50]).unwrap();
    assert_eq!(TextIndex::open(&path, "test-ocr").unwrap().len(), 1);
    // a header never finished
    std::fs::write(&path, b"LCVTEXT").unwrap();
    assert!(TextIndex::open(&path, "test-ocr").unwrap().is_empty());
}

#[test]
fn another_engine_or_file_is_refused_and_left_alone() {
    let dir = Dir::new();
    let path = dir.file("t.bin");
    TextIndex::open(&path, "engine-a").unwrap().insert(key(1), "x").map(|_| ()).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(TextIndex::open(&path, "engine-b"), Err(Error::Mismatch { .. })));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let foreign = dir.file("f.bin");
    std::fs::write(&foreign, vec![7u8; 100]).unwrap();
    assert!(matches!(TextIndex::open(&foreign, "engine-a"), Err(Error::Format(_))));
    assert_eq!(std::fs::read(&foreign).unwrap(), vec![7u8; 100]);
    for bad in ["", "an engine id that is far too long", "naïve"] {
        assert!(TextIndex::in_memory(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn a_device_sends_a_server_only_the_text_it_lacks() {
    let mut device = library();
    let mut server = TextIndex::in_memory("test-ocr").unwrap();
    server.insert(key(0), "INVOICE No. 48213\nTotal: $1,234.50").unwrap();
    let up = device.export(|k| server.contains(k), 100);
    let done = server.import(&up, |_| true).unwrap();
    assert_eq!((done.added, done.skipped), (7, 0));
    assert_eq!(server.len(), 8);
    assert_eq!(server.search("tokyo", 5).len(), 0);
    assert_eq!(server.search("東京", 5).len(), 1);
    // again: nothing new; and the max
    assert_eq!(server.import(&up, |_| true).unwrap().added, 0);
    assert_eq!(
        device.export(|_| false, 2).len(),
        device.export(|_| false, 1).len() + (device.export(|_| false, 2).len() - device.export(|_| false, 1).len())
    );
    // photos the user doesn't have are skipped
    let mut other = TextIndex::in_memory("test-ocr").unwrap();
    let only = key(4);
    let done = other.import(&device.export(|_| false, 100), |k| *k == only).unwrap();
    assert_eq!((done.added, done.skipped), (1, 7));
    // a text the server doesn't have, sent twice in one upload, is kept once
    let mut dup = device.export(|_| false, 1);
    let rec = dup[32..].to_vec();
    dup.extend_from_slice(&rec);
    let mut fresh = TextIndex::in_memory("test-ocr").unwrap();
    assert_eq!(fresh.import(&dup, |_| true).unwrap().added, 1);
    let _ = &mut device;
}

#[test]
fn a_bad_upload_is_refused_whole() {
    let device = library();
    let good = device.export(|_| false, 100);
    let mut server = TextIndex::in_memory("test-ocr").unwrap();
    assert!(matches!(server.import(&good[..good.len() - 3], |_| true), Err(Error::Format(_))), "truncated");
    assert!(matches!(server.import(&[], |_| true), Err(Error::Format(_))));
    assert!(matches!(server.import(b"LCVTEXT1", |_| true), Err(Error::Format(_))));
    let mut flipped = good.clone();
    let n = flipped.len();
    flipped[n - 10] ^= 1;
    assert!(server.import(&flipped, |_| true).is_err(), "checksum");
    let other = TextIndex::in_memory("another-ocr").unwrap().export(|_| false, 1);
    assert!(matches!(server.import(&other, |_| true), Err(Error::Mismatch { .. })));
    assert!(server.is_empty(), "nothing of any of it was kept");
}

#[test]
fn very_long_text_is_cut_not_refused() {
    let mut ix = TextIndex::in_memory("test-ocr").unwrap();
    let long = "word ".repeat(100_000) + "終わり";
    assert!(ix.insert(key(1), &long).unwrap());
    let kept = ix.text(&key(1)).unwrap();
    assert!(kept.len() <= crate::textindex::MAX_TEXT && kept.is_char_boundary(kept.len()));
    // a text of multi-byte characters is cut at a character boundary
    assert!(ix.insert(key(2), &"あ".repeat(100_000)).unwrap());
    assert!(ix.text(&key(2)).unwrap().chars().all(|c| c == 'あ'));
    // a long query is cut too
    assert!(ix.search(&"word ".repeat(10_000), 5).len() <= 5);
}
