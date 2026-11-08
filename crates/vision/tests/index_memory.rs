//! What opening a library's search indexes costs before anything is searched (HB-273): their photos' keys, not the
//! photos' vectors (1.5 KB a photo) or faces, which the first search or People view reads from the file.

use lightcraft_sysmem::{CountingAlloc, counted};
use lightcraft_vision::faces::index::FaceIndex;
use lightcraft_vision::faces::{DIM as FACE_DIM, FaceFound};
use lightcraft_vision::{EmbeddingIndex, Key};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const PHOTOS: usize = 4000;
const DIM: usize = 768;
const MODEL: &str = "test-model";

/// A deterministic pseudo-random vector.
fn vector(seed: usize, dim: usize) -> Vec<f32> {
    let mut s = (seed as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
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

#[test]
fn opening_an_index_reads_its_keys_and_a_search_the_rest() {
    let dir = std::env::temp_dir().join(format!("lc-index-memory-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (vectors, faces) = (dir.join("vectors.bin"), dir.join("faces.bin"));
    let mut all = EmbeddingIndex::in_memory(MODEL, DIM).unwrap();
    {
        let mut ix = EmbeddingIndex::open(&vectors, MODEL, DIM).unwrap();
        let mut fx = FaceIndex::open(&faces).unwrap();
        for i in 0..PHOTOS {
            ix.insert(key(i), &vector(i, DIM)).unwrap();
            all.insert(key(i), &vector(i, DIM)).unwrap();
            fx.insert_photo(key(i), &[FaceFound { rect: [0.2, 0.2, 0.5, 0.6], score: 0.9, embedding: vector(i, FACE_DIM) }]).unwrap();
        }
    }

    let before = counted().0;
    let ix = EmbeddingIndex::open(&vectors, MODEL, DIM).unwrap();
    let fx = FaceIndex::open(&faces).unwrap();
    let opened = counted().0.saturating_sub(before);
    assert_eq!((ix.len(), fx.len(), fx.photos()), (PHOTOS, PHOTOS, PHOTOS));
    // (the keys and where they are: ~100 B a photo; the vectors alone are 1,536 B and a face 256 B more)
    assert!(opened / PHOTOS < 256, "{} B a photo after opening", opened / PHOTOS);

    let hits = ix.search(&vector(7, DIM), 5).unwrap();
    assert_eq!(hits, all.search(&vector(7, DIM), 5).unwrap(), "the answers of an index that held everything");
    assert_eq!(hits.first().map(|h| h.key), Some(key(7)));
    assert_eq!(fx.faces().len(), PHOTOS);
    let read = counted().0.saturating_sub(before);
    println!("{PHOTOS} photos: {} B a photo once opened, {} B once searched", opened / PHOTOS, read / PHOTOS);
    assert!(read / PHOTOS >= DIM * 2 + FACE_DIM * 2, "{} B a photo once searched", read / PHOTOS);
    drop((ix, fx));
    let _ = std::fs::remove_dir_all(&dir);
}
