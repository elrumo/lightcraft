use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::index::f16_to_f32;
use crate::store::{HEADER_LEN, KEY_LEN, Spec};
use crate::{EmbeddingIndex, Error, Key};

const DIM: usize = 16;
const MODEL: &str = "test-model";

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A scratch directory removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        let d = std::env::temp_dir().join(format!("lc-vision-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
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

/// A deterministic pseudo-random vector.
fn vec_of(seed: u64, dim: usize) -> Vec<f32> {
    let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (0..dim)
        .map(|_| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (s >> 33) as f32 / (1u64 << 31) as f32 - 0.5
        })
        .collect()
}

fn key(i: usize) -> Key {
    Key::of(&format!("photo-{i}"))
}

fn filled(path: &std::path::Path, n: usize) -> EmbeddingIndex {
    let mut ix = EmbeddingIndex::open(path, MODEL, DIM).unwrap();
    for i in 0..n {
        assert!(ix.insert(key(i), &vec_of(i as u64, DIM)).unwrap());
    }
    ix
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let (na, nb) = (a.iter().map(|x| x * x).sum::<f32>().sqrt(), b.iter().map(|x| x * x).sum::<f32>().sqrt());
    dot / (na * nb)
}

fn file_len(p: &std::path::Path) -> u64 {
    std::fs::metadata(p).unwrap().len()
}

const REC: u64 = (KEY_LEN + DIM * 2) as u64;

/// One record as it sits in a file or an upload.
fn record(key: Key, bits: &[u16]) -> Vec<u8> {
    let mut r = key.0.to_vec();
    for b in bits {
        r.extend_from_slice(&b.to_le_bytes());
    }
    r
}

fn header() -> [u8; HEADER_LEN] {
    Spec { model: MODEL, dim: DIM, rec_len: KEY_LEN + DIM * 2 }.header()
}

#[test]
fn f16_decoding_matches_the_half_crate_for_every_finite_value() {
    for h in 0..=u16::MAX {
        if h & 0x7c00 == 0x7c00 {
            continue;
        }
        assert_eq!(f16_to_f32(h), half::f16::from_bits(h).to_f32(), "bits {h:#06x}");
    }
}

#[test]
fn keys_round_trip_and_only_real_hashes_are_taken_literally() {
    let k = Key::of("0123456789abcdef0123456789ABCDEF");
    assert_eq!(k.to_hex(), "0123456789abcdef0123456789abcdef");
    assert_eq!(Key::from_hex(&k.to_hex()), Some(k));
    assert_eq!(Key::of("demo:3"), Key::of("demo:3"));
    assert_ne!(Key::of("demo:3"), Key::of("demo:4"));
    // 32 bytes that are not 32 hex digits: a sign, a multi-byte character, a short hash
    assert_eq!(Key::from_hex("+0000000000000000000000000000000"), None);
    assert_eq!(Key::from_hex(&format!("{}é", "0".repeat(30))), None);
    assert_eq!(Key::from_hex("abc"), None);
    assert_ne!(Key::of("+0000000000000000000000000000000"), Key::of("00000000000000000000000000000000"));
}

#[test]
fn vectors_are_found_and_survive_reopening() {
    let dir = Dir::new();
    let path = dir.file("e.bin");
    let ix = filled(&path, 50);
    let hits = ix.search(&vec_of(7, DIM), 5).unwrap();
    assert_eq!(hits.len(), 5);
    assert_eq!(hits[0].key, key(7));
    assert!(hits[0].score > 0.999, "{}", hits[0].score);
    assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
    // the scores are the cosines, to f16 precision
    for h in &hits {
        let i = (0..50).find(|&i| key(i) == h.key).unwrap();
        assert!((h.score - cosine(&vec_of(7, DIM), &vec_of(i as u64, DIM))).abs() < 2e-3);
    }
    drop(ix);

    let again = EmbeddingIndex::open(&path, MODEL, DIM).unwrap();
    assert_eq!(again.len(), 50);
    assert_eq!(again.search(&vec_of(7, DIM), 5).unwrap(), hits);
}

#[test]
fn asking_for_more_than_there_is_returns_everything_in_order() {
    let dir = Dir::new();
    let ix = filled(&dir.file("e.bin"), 5);
    assert_eq!(ix.search(&vec_of(1, DIM), 100).unwrap().len(), 5);
    assert!(ix.search(&vec_of(1, DIM), 0).unwrap().is_empty());
    let empty = EmbeddingIndex::open(&dir.file("empty.bin"), MODEL, DIM).unwrap();
    assert!(empty.search(&vec_of(1, DIM), 10).unwrap().is_empty());
}

#[test]
fn a_photo_is_stored_once() {
    let dir = Dir::new();
    let path = dir.file("e.bin");
    let mut ix = filled(&path, 3);
    let before = file_len(&path);
    assert!(!ix.insert(key(1), &vec_of(99, DIM)).unwrap());
    assert_eq!(file_len(&path), before);
    assert_eq!(ix.len(), 3);
}

#[test]
fn a_partial_record_at_the_end_is_cut_off() {
    let dir = Dir::new();
    let path = dir.file("e.bin");
    drop(filled(&path, 4));
    let whole = file_len(&path);
    assert_eq!(whole, HEADER_LEN as u64 + 4 * REC);
    OpenOptions::new().append(true).open(&path).unwrap().write_all(&[1, 2, 3, 4, 5]).unwrap();

    let mut ix = EmbeddingIndex::open(&path, MODEL, DIM).unwrap();
    assert_eq!(ix.len(), 4);
    assert_eq!(file_len(&path), whole);
    // and appending after the cut stays aligned
    assert!(ix.insert(key(10), &vec_of(10, DIM)).unwrap());
    drop(ix);
    assert_eq!(EmbeddingIndex::open(&path, MODEL, DIM).unwrap().len(), 5);
    assert_eq!(file_len(&path), whole + REC);
}

#[test]
fn a_header_that_was_never_finished_starts_over() {
    let dir = Dir::new();
    let path = dir.file("e.bin");
    std::fs::write(&path, b"LCVSTOR").unwrap();
    let ix = EmbeddingIndex::open(&path, MODEL, DIM).unwrap();
    assert!(ix.is_empty());
    assert_eq!(file_len(&path), HEADER_LEN as u64);
}

#[test]
fn other_files_and_other_models_are_refused_and_left_alone() {
    let dir = Dir::new();
    let foreign = dir.file("foreign.bin");
    std::fs::write(&foreign, vec![7u8; 200]).unwrap();
    assert!(matches!(EmbeddingIndex::open(&foreign, MODEL, DIM), Err(Error::Format(_))));
    assert_eq!(std::fs::read(&foreign).unwrap(), vec![7u8; 200]);

    let path = dir.file("e.bin");
    drop(filled(&path, 3));
    let len = file_len(&path);
    assert!(matches!(EmbeddingIndex::open(&path, "other-model", DIM), Err(Error::Mismatch { .. })));
    assert!(matches!(EmbeddingIndex::open(&path, MODEL, DIM + 1), Err(Error::Mismatch { .. })));
    assert_eq!(file_len(&path), len);
}

#[test]
fn a_header_cannot_make_the_loader_allocate() {
    let dir = Dir::new();
    let path = dir.file("e.bin");
    let mut h = header();
    h[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    h[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut bytes = h.to_vec();
    bytes.extend_from_slice(&[0u8; 100]);
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(EmbeddingIndex::open(&path, MODEL, DIM), Err(Error::Mismatch { .. })));
    // and a caller asking for a huge or empty vector is refused outright
    assert!(matches!(EmbeddingIndex::open(&dir.file("big.bin"), MODEL, usize::MAX), Err(Error::Invalid(_))));
    assert!(matches!(EmbeddingIndex::open(&dir.file("zero.bin"), MODEL, 0), Err(Error::Invalid(_))));
    assert!(matches!(EmbeddingIndex::open(&dir.file("name.bin"), "a-model-id-that-is-far-too-long", DIM), Err(Error::Invalid(_))));
}

#[test]
fn bad_vectors_are_refused_and_change_nothing() {
    let dir = Dir::new();
    let path = dir.file("e.bin");
    let mut ix = filled(&path, 1);
    let before = file_len(&path);
    let mut nan = vec_of(2, DIM);
    nan[3] = f32::NAN;
    let mut inf = vec_of(2, DIM);
    inf[0] = f32::INFINITY;
    for bad in [vec![0.0; DIM], nan, inf, vec_of(2, DIM + 1), vec![]] {
        assert!(matches!(ix.insert(key(2), &bad), Err(Error::Invalid(_))), "{bad:?}");
    }
    assert_eq!(ix.len(), 1);
    assert_eq!(file_len(&path), before);
    assert!(ix.search(&vec_of(1, DIM + 1), 3).is_err());
    assert!(ix.search(&[f32::NAN; DIM], 3).is_err());
    assert!(ix.search(&[0.0; DIM], 3).is_err());
}

#[test]
fn a_damaged_record_is_ignored_and_the_rest_still_load() {
    let dir = Dir::new();
    let path = dir.file("e.bin");
    drop(filled(&path, 2));
    let mut f = OpenOptions::new().append(true).open(&path).unwrap();
    f.write_all(&record(key(50), &[0x7e00; DIM])).unwrap(); // NaN
    f.write_all(&record(key(51), &[0; DIM])).unwrap(); // no length
    f.write_all(&record(key(0), &[0x3c00; DIM])).unwrap(); // a second vector for a photo
    drop(f);
    let ix = EmbeddingIndex::open(&path, MODEL, DIM).unwrap();
    assert_eq!(ix.len(), 2);
    assert!(!ix.contains(&key(50)) && !ix.contains(&key(51)));
    // the first vector for a photo wins
    assert!(cosine(&ix.vector(&key(0)).unwrap(), &vec_of(0, DIM)) > 0.999);
}

#[test]
fn a_device_sends_a_server_only_what_it_lacks() {
    let dir = Dir::new();
    let mut device = filled(&dir.file("device.bin"), 6);
    let mut server = EmbeddingIndex::open(&dir.file("server.bin"), MODEL, DIM).unwrap();
    for i in [1, 4] {
        server.insert(key(i), &vec_of(i as u64, DIM)).unwrap();
    }
    server.insert(key(100), &vec_of(100, DIM)).unwrap();

    let upload = device.export(|k| server.contains(k), 100);
    assert_eq!(upload.len(), HEADER_LEN + 4 * (REC as usize));
    assert_eq!(device.export(|k| server.contains(k), 2).len(), HEADER_LEN + 2 * (REC as usize));

    let done = server.import(&upload, |_| true).unwrap();
    assert_eq!((done.added, done.skipped), (4, 0));
    assert_eq!(server.len(), 7);
    // sending everything again changes nothing
    let again = server.import(&device.export(|_| false, 100), |_| true).unwrap();
    assert_eq!((again.added, again.skipped), (0, 6));
    // and the other way round: the server's photo the device lacks
    let back = device.import(&server.export(|k| device.contains(k), 100), |_| true).unwrap();
    assert_eq!(back.added, 1);
    assert!(device.contains(&key(100)));

    drop(server);
    let reopened = EmbeddingIndex::open(&dir.file("server.bin"), MODEL, DIM).unwrap();
    assert_eq!(reopened.len(), 7);
    assert_eq!(reopened.search(&vec_of(3, DIM), 1).unwrap()[0].key, key(3));
}

#[test]
fn an_upload_for_photos_the_user_does_not_have_is_skipped() {
    let dir = Dir::new();
    let device = filled(&dir.file("device.bin"), 4);
    let mut server = EmbeddingIndex::open(&dir.file("server.bin"), MODEL, DIM).unwrap();
    let mine = [key(0), key(2)];
    let done = server.import(&device.export(|_| false, 100), |k| mine.contains(k)).unwrap();
    assert_eq!((done.added, done.skipped), (2, 2));
    assert!(server.contains(&key(0)) && !server.contains(&key(1)));
}

#[test]
fn a_bad_upload_is_rejected_whole() {
    let dir = Dir::new();
    let path = dir.file("server.bin");
    let mut server = filled(&path, 2);
    let before = file_len(&path);
    let good = record(key(20), &[0x3c00; DIM]);

    let with = |extra: &[u8]| {
        let mut up = header().to_vec();
        up.extend_from_slice(&good);
        up.extend_from_slice(extra);
        up
    };
    let mut other_model = Spec { model: "other-model", dim: DIM, rec_len: KEY_LEN + DIM * 2 }.header().to_vec();
    other_model.extend_from_slice(&good);
    let mut truncated = with(&record(key(21), &[0x3c00; DIM]));
    truncated.truncate(truncated.len() - 3);

    assert!(matches!(server.import(&other_model, |_| true), Err(Error::Mismatch { .. })));
    assert!(matches!(server.import(&truncated, |_| true), Err(Error::Format(_))));
    assert!(matches!(server.import(&with(&record(key(21), &[0x7e00; DIM])), |_| true), Err(Error::Invalid(_))));
    assert!(matches!(server.import(&with(&record(key(21), &[0x7c00; DIM])), |_| true), Err(Error::Invalid(_))));
    assert!(matches!(server.import(&with(&record(key(21), &[0; DIM])), |_| true), Err(Error::Invalid(_))));
    assert!(matches!(server.import(&[], |_| true), Err(Error::Format(_))));
    assert!(matches!(server.import(&b"LCVSTOR1"[..], |_| true), Err(Error::Format(_))));
    // the good record that came before each bad one was not kept either
    assert_eq!(server.len(), 2);
    assert!(!server.contains(&key(20)));
    assert_eq!(file_len(&path), before);
}

#[test]
fn uploaded_vectors_are_rescaled_so_they_cannot_dominate_a_ranking() {
    let dir = Dir::new();
    let mut server = filled(&dir.file("server.bin"), 1);
    let mut up = header().to_vec();
    up.extend_from_slice(&record(key(30), &[0x5c00; DIM])); // every entry 256.0
    server.import(&up, |_| true).unwrap();
    let v = server.vector(&key(30)).unwrap();
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 2e-3, "{norm}");
}

/// `cargo test -p lightcraft-vision --release -- --ignored --nocapture`: search time at the size
/// the design assumes (100k photos, SigLIP 2's 768 dimensions).
#[test]
#[ignore]
fn search_over_a_large_library_is_fast() {
    const N: usize = 100_000;
    const D: usize = 768;
    let dir = Dir::new();
    let mut ix = EmbeddingIndex::open(&dir.file("big.bin"), MODEL, D).unwrap();
    let t = std::time::Instant::now();
    for i in 0..N {
        ix.insert(key(i), &vec_of(i as u64, D)).unwrap();
    }
    println!("insert {N}: {:?}", t.elapsed());
    let q = vec_of(4242, D);
    let t = std::time::Instant::now();
    let hits = ix.search(&q, 100).unwrap();
    println!("search {N} x {D}: {:?}", t.elapsed());
    assert_eq!(hits[0].key, key(4242));
    let t = std::time::Instant::now();
    drop(ix);
    let ix = EmbeddingIndex::open(&dir.file("big.bin"), MODEL, D).unwrap();
    println!("reopen {N}: {:?}", t.elapsed());
    assert_eq!(ix.len(), N);
}
