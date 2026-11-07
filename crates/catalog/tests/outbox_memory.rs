//! What the sync outbox costs when a whole library is queued for a server (a library's first sync, HB-270): the heap
//! it holds and the time it takes. A queued photo is its id, not a copy of the photo, and queueing is linear.
//!
//! The 200,000-photo numbers: `cargo test --release -p lightcraft-catalog --test outbox_memory -- --ignored --nocapture`.

use std::time::Instant;

use lightcraft_catalog::sync::Outbox;
use lightcraft_catalog::{Catalog, Op, Photo, PhotoId, Source};
use lightcraft_sysmem::{CountingAlloc, counted};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// `n` photos as a camera library has them: files on this disk with their content hashes.
fn library(n: u64) -> Catalog {
    let mut c = Catalog::new();
    for i in 0..n {
        let name = format!("IMG_{i:06}.CR3");
        let mut p = Photo::new(
            PhotoId(i + 1),
            Source::File { path: format!("/Volumes/Photos/2026/{name}") },
            &name,
            "CR3",
            6000,
            4000,
            "2026-01-01T00:00:00",
        );
        p.content_hash = Some(format!("{:032x}", u128::from(i) * 0x9e37_79b9_7f4a_7c15));
        p.captured = Some("2025-06-01T12:00:00".into());
        c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    c
}

/// Seed an outbox with `c`: the heap it holds (bytes), the seconds it took, and the outbox.
fn seed(c: &Catalog) -> (usize, f64, Outbox) {
    let before = counted().0;
    let t = Instant::now();
    let mut out = Outbox::default();
    out.seed(c);
    let secs = t.elapsed().as_secs_f64();
    (counted().0.saturating_sub(before), secs, out)
}

#[test]
fn a_queued_photo_is_its_id_not_a_copy() {
    let c = library(20_000);
    let (bytes, _, out) = seed(&c);
    assert_eq!(out.len(), 20_000);
    let per_photo = bytes / 20_000;
    // (a copy of each photo was ~3 KB; the slot and its index entry are ~50 B, ~100 B with the containers' spare room)
    assert!(per_photo <= 128, "{per_photo} B a photo");
}

#[test]
#[ignore]
fn seed_200k() {
    for n in [50_000, 100_000, 200_000] {
        let c = library(n);
        let (bytes, secs, out) = seed(&c);
        let json = serde_json::to_vec(&out).unwrap().len();
        println!(
            "{n} photos: outbox {:.1} MB ({} B a photo), seeded in {:.3} s, file {:.1} MB",
            bytes as f64 / 1_048_576.0,
            bytes / n as usize,
            secs,
            json as f64 / 1_048_576.0
        );
    }
}
