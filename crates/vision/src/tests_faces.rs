use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::faces::cluster::{self, face_id, parse_face_id};
use crate::faces::index::{FaceIndex, REC_LEN};
use crate::faces::{DIM, FaceFound};
use crate::store::HEADER_LEN;
use crate::{Error, Key};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Dir(PathBuf);

impl Dir {
    fn new() -> Dir {
        let d = std::env::temp_dir().join(format!("lc-faces-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
    fn file(&self) -> PathBuf {
        self.0.join("faces.bin")
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A repeatable pseudo-random number in -1..1.
fn noise(seed: &mut u64) -> f32 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*seed >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
}

/// A person's vector: a fixed direction per identity, `jitter` of noise.
fn person(id: u64, jitter: f32, seed: &mut u64) -> Vec<f32> {
    let mut base = id.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..DIM).map(|_| noise(&mut base) + jitter * noise(seed)).collect()
}

fn found(id: u64, jitter: f32, seed: &mut u64) -> FaceFound {
    FaceFound { rect: [0.2, 0.2, 0.5, 0.6], score: 0.95, embedding: person(id, jitter, seed) }
}

fn key(i: usize) -> Key {
    Key::of(&format!("photo-{i}"))
}

#[test]
fn faces_are_kept_per_photo_and_survive_a_restart() {
    let dir = Dir::new();
    let mut seed = 1;
    let mut ix = FaceIndex::open(&dir.file()).unwrap();
    assert!(ix.insert_photo(key(1), &[found(1, 0.1, &mut seed), found(2, 0.1, &mut seed)]).unwrap());
    assert!(ix.insert_photo(key(2), &[]).unwrap(), "a photo with nobody in it is marked as looked at");
    assert!(!ix.insert_photo(key(1), &[found(3, 0.1, &mut seed)]).unwrap(), "a photo is looked at once");
    // faces that can't be used are left out, not stored
    let bad = [
        FaceFound { embedding: vec![f32::NAN; DIM], ..found(1, 0.0, &mut seed) },
        FaceFound { embedding: vec![0.0; DIM], ..found(1, 0.0, &mut seed) },
        FaceFound { embedding: vec![1.0; 3], ..found(1, 0.0, &mut seed) },
        FaceFound { rect: [0.5, 0.5, 0.2, 0.2], ..found(1, 0.0, &mut seed) },
        FaceFound { rect: [f32::INFINITY, 0.0, 1.0, 1.0], ..found(1, 0.0, &mut seed) },
        found(4, 0.1, &mut seed),
    ];
    assert!(ix.insert_photo(key(3), &bad).unwrap());
    assert_eq!((ix.len(), ix.photos()), (3, 3));
    assert!(ix.faces().iter().all(|f| (f.embedding.iter().map(|x| x * x).sum::<f32>().sqrt() - 1.0).abs() < 1e-3), "unit length");
    let before: Vec<_> = ix.faces().to_vec();
    drop(ix);

    let again = FaceIndex::open(&dir.file()).unwrap();
    assert_eq!((again.len(), again.photos()), (3, 3));
    assert!(again.scanned(&key(2)) && !again.scanned(&key(9)));
    for (a, b) in again.faces().iter().zip(&before) {
        assert_eq!((a.key, a.index, a.rect), (b.key, b.index, b.rect));
        // (f16 storage: close, not equal)
        assert!(a.embedding.iter().zip(&b.embedding).all(|(x, y)| (x - y).abs() < 2e-3));
    }
}

#[test]
fn a_photo_that_was_cut_off_half_way_is_not_believed() {
    let dir = Dir::new();
    let mut seed = 2;
    let mut ix = FaceIndex::open(&dir.file()).unwrap();
    ix.insert_photo(key(1), &[found(1, 0.1, &mut seed)]).unwrap();
    ix.insert_photo(key(2), &[found(2, 0.1, &mut seed), found(3, 0.1, &mut seed), found(4, 0.1, &mut seed)]).unwrap();
    drop(ix);
    let full = std::fs::metadata(dir.file()).unwrap().len();
    assert_eq!(full as usize, HEADER_LEN + 4 * REC_LEN);
    // the last record never made it: photo 2 has two of its three faces, so it is as if unseen
    let f = std::fs::OpenOptions::new().write(true).open(dir.file()).unwrap();
    f.set_len(full - REC_LEN as u64).unwrap();
    drop(f);
    let ix = FaceIndex::open(&dir.file()).unwrap();
    assert_eq!((ix.len(), ix.photos()), (1, 1));
    assert!(ix.scanned(&key(1)) && !ix.scanned(&key(2)));
    // a cut inside a record is cut off
    let f = std::fs::OpenOptions::new().write(true).open(dir.file()).unwrap();
    f.set_len(full - 100).unwrap();
    drop(f);
    let mut ix = FaceIndex::open(&dir.file()).unwrap();
    assert_eq!(ix.photos(), 1);
    // and the photo can be looked at again
    assert!(ix.insert_photo(key(2), &[found(2, 0.1, &mut seed)]).unwrap());
}

#[test]
fn another_file_is_refused_and_left_alone() {
    let dir = Dir::new();
    std::fs::write(dir.file(), b"this is somebody's text file, not a face index at all").unwrap();
    assert!(matches!(FaceIndex::open(&dir.file()), Err(Error::Format(_) | Error::Mismatch { .. })));
    assert_eq!(std::fs::read(dir.file()).unwrap(), b"this is somebody's text file, not a face index at all");
}

#[test]
fn faces_move_between_indexes_whole_or_not_at_all() {
    let mut seed = 3;
    let mut a = FaceIndex::in_memory();
    a.insert_photo(key(1), &[found(1, 0.1, &mut seed), found(2, 0.1, &mut seed)]).unwrap();
    a.insert_photo(key(2), &[]).unwrap();
    a.insert_photo(key(3), &[found(1, 0.1, &mut seed)]).unwrap();
    let bytes = a.export(|k| *k == key(3), 10);
    assert_eq!(FaceIndex::exported_keys(&bytes).len(), 2, "what the other side has is left out");

    let mut b = FaceIndex::in_memory();
    let r = b.import(&bytes, |k| *k != key(2)).unwrap();
    assert_eq!((r.added, r.skipped), (1, 1));
    assert_eq!((b.len(), b.photos()), (2, 1));
    let again = b.import(&bytes, |_| true).unwrap();
    assert_eq!((again.added, again.skipped), (1, 1), "photo 1 is there; the one that wasn't allowed comes now");
    assert_eq!((b.len(), b.photos()), (2, 2));
    assert_eq!(FaceIndex::exported_keys(&a.export(|_| false, 1)).len(), 1, "at most `max` photos");

    // anything wrong rejects the lot
    let mut c = FaceIndex::in_memory();
    let good = a.export(|_| false, 10);
    assert!(c.import(&good[..good.len() - 5], |_| true).is_err(), "cut off");
    assert!(c.import(&good[..good.len() - REC_LEN], |_| true).is_err(), "a photo without all its faces");
    assert!(c.import(b"nope", |_| true).is_err());
    assert!(c.import(&[], |_| true).is_err());
    let mut nan = good.clone();
    // (in a record of a face, not in the marker of a photo with none)
    let rec = (0..4).find(|r| good[HEADER_LEN + r * REC_LEN + 17] == 0).unwrap();
    let at = HEADER_LEN + rec * REC_LEN + 24;
    nan[at..at + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(c.import(&nan, |_| true).is_err(), "a rectangle that isn't a number");
    let mut doubled = good.clone();
    doubled.extend_from_slice(&good[HEADER_LEN..]);
    assert!(c.import(&doubled, |_| true).is_err(), "a photo twice");
    assert_eq!((c.len(), c.photos()), (0, 0), "nothing was kept");
    // junk never panics
    let mut s = 9u64;
    for _ in 0..200 {
        let junk: Vec<u8> = (0..HEADER_LEN + REC_LEN * 2).map(|_| (noise(&mut s) * 127.0) as u8).collect();
        let _ = c.import(&junk, |_| true);
    }
}

#[test]
fn clearing_forgets_everything_on_disk_too() {
    let dir = Dir::new();
    let mut seed = 4;
    let mut ix = FaceIndex::open(&dir.file()).unwrap();
    ix.insert_photo(key(1), &[found(1, 0.1, &mut seed)]).unwrap();
    let rev = ix.revision();
    ix.clear().unwrap();
    assert!(ix.is_empty() && ix.photos() == 0 && ix.revision() > rev);
    assert_eq!(std::fs::metadata(dir.file()).unwrap().len() as usize, HEADER_LEN);
    assert!(FaceIndex::open(&dir.file()).unwrap().is_empty());
    assert!(ix.insert_photo(key(1), &[found(1, 0.1, &mut seed)]).unwrap(), "and it can be looked at again");
}

#[test]
fn faces_of_the_same_person_make_one_person() {
    let mut seed = 5;
    let mut ix = FaceIndex::in_memory();
    // Ann in 6 photos, Ben in 4, Cy in 2, a stranger once; Ann and Ben together in two of them
    let mut photo = 0;
    for (who, n) in [(1u64, 6usize), (2, 4), (3, 2), (4, 1)] {
        for _ in 0..n {
            ix.insert_photo(key(photo), &[found(who, 0.25, &mut seed)]).unwrap();
            photo += 1;
        }
    }
    ix.insert_photo(key(100), &[found(1, 0.25, &mut seed), found(2, 0.25, &mut seed)]).unwrap();
    let groups = cluster::group(ix.faces(), cluster::JOIN);
    let sizes: Vec<usize> = groups.iter().map(|g| g.members.len()).collect();
    assert_eq!(sizes, [7, 5, 2, 1], "{sizes:?}");
    // the same faces always make the same people
    assert_eq!(cluster::group(ix.faces(), cluster::JOIN), groups);
    // each person is one identity: all their faces are the same person's
    for g in &groups {
        let who = |i: usize| {
            ix.faces().get(i).map(|f| {
                (0..4u64).max_by(|&a, &b| {
                    let (da, db): (f32, f32) = (dot(&f.embedding, &person(a + 1, 0.0, &mut 0)), dot(&f.embedding, &person(b + 1, 0.0, &mut 0)));
                    da.total_cmp(&db)
                })
            })
        };
        let first = who(g.members[0]);
        assert!(g.members.iter().all(|&m| who(m) == first));
    }
    // a rule that lets anyone join joins strangers
    assert_eq!(cluster::group(ix.faces(), -1.0).len(), 1);
    assert!(cluster::group(&[], cluster::JOIN).is_empty());
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    let n = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() / (n(a) * n(b))
}

#[test]
fn a_person_is_named_by_their_best_face() {
    let mut seed = 6;
    let mut ix = FaceIndex::in_memory();
    ix.insert_photo(key(1), &[found(1, 0.1, &mut seed)]).unwrap();
    let f = &ix.faces()[0];
    let id = face_id(f);
    assert_eq!(parse_face_id(&id), Some((key(1), 0)));
    for bad in ["", ".", "zz.0", &format!("{}.", key(1).to_hex()), &format!("{}.300", key(1).to_hex()), &format!("{}.-1", key(1).to_hex())] {
        assert_eq!(parse_face_id(bad), None, "{bad:?}");
    }
}
