use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use lightcraft_pipeline::{SourceInfo, tone::CameraTone};
use serde_json::{Value, json};

use super::*;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-facts-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn tone() -> CameraTone {
    CameraTone::new(std::array::from_fn(|i| {
        let x = 0.004 * 1.17f32.powi(i as i32);
        [x, x / (1.0 + x)]
    }))
    .unwrap()
    .with_chroma([1.1, 1.05, 1.0, 0.98, 0.95, 0.9, 0.85, 0.8])
    .unwrap()
}

fn lens() -> EmbeddedLens {
    serde_json::from_value(
        json!({"vignette": {"k": [0.31, -0.02, 0.004, 0.0, 0.0], "center": {"x": 0.5, "y": 0.49}, "radius": 1.0}, "origin": "measured"}),
    )
    .unwrap()
}

fn facts(key: u64) -> StoredFacts {
    StoredFacts {
        key,
        lens: Some(lens()),
        camera_tone: Some(tone()),
        level: Some(std::array::from_fn(|i| 0.01 * (i + 1) as f32)),
        colour: Some((0..96).map(|i| (i % 7 != 3).then(|| [0.1 + i as f32 / 300.0, 0.2, 0.3 - i as f32 / 1000.0])).collect()),
        camera_match: true,
        local_tone: false,
        mattes: false,
    }
}

#[test]
fn a_record_reads_back_as_it_was_written() {
    for f in [facts(7), StoredFacts { lens: None, camera_tone: None, level: None, colour: None, camera_match: false, ..facts(8) }] {
        let record = encode(PhotoId(42), &f).unwrap();
        assert_eq!(frame(&record, 0), Some((42, f.key, record.len())));
        assert_eq!(decode(&record[HEADER..]), Some(f));
    }
}

#[test]
fn a_damaged_or_cut_short_record_is_not_read() {
    let record = encode(PhotoId(1), &facts(3)).unwrap();
    // a flipped bit anywhere: the checksum (or the length) no longer fits
    for i in [0, 5, HEADER, HEADER + 17, record.len() - 1] {
        let mut bad = record.clone();
        bad[i] ^= 0x10;
        assert!(frame(&bad, 0).is_none(), "byte {i}");
    }
    for n in [0, 3, HEADER, record.len() - 1] {
        assert!(frame(&record[..n], 0).is_none(), "{n} bytes");
    }
    // a payload that checks out but holds nonsense (a curve going down) is refused when read
    let mut payload = record[HEADER..].to_vec();
    let knot = 8 + 8 + 1 + 4;
    payload[knot..knot + 4].copy_from_slice(&(-1.0f32).to_le_bytes());
    assert_eq!(decode(&payload), None);
    // and so is one with bytes left over
    let mut longer = record[HEADER..].to_vec();
    longer.push(0);
    assert_eq!(decode(&longer), None);
}

#[test]
fn records_are_found_again_and_the_last_one_wins() {
    let dir = temp_dir("store");
    let mut store = FactsStore::default();
    store.attach(dir.clone(), None);
    // consecutive photos share a shard; one far away has its own
    for id in [1, 2, 64 * 300 + 5] {
        store.put(PhotoId(id), facts(id * 10));
    }
    store.put(PhotoId(2), facts(21));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);

    let mut again = FactsStore::default();
    again.attach(dir.clone(), None);
    assert_eq!(again.get(PhotoId(1), 10).as_deref(), Some(&facts(10)));
    assert_eq!(again.get(PhotoId(2), 20), None, "replaced");
    assert_eq!(again.get(PhotoId(2), 21).as_deref(), Some(&facts(21)));
    assert_eq!(again.get(PhotoId(64 * 300 + 5), 64 * 300 * 10 + 50).as_deref(), Some(&facts(64 * 300 * 10 + 50)));
    assert_eq!(again.get(PhotoId(3), 30), None);
    // the same facts again write nothing
    let shard = dir.join("00.lcf");
    let before = std::fs::metadata(&shard).unwrap().len();
    again.put(PhotoId(1), facts(10));
    assert_eq!(std::fs::metadata(&shard).unwrap().len(), before);
    // a detached store keeps and finds nothing
    let mut none = FactsStore::default();
    none.put(PhotoId(1), facts(10));
    none.clear_memo();
    assert_eq!(none.get(PhotoId(1), 10), None);
}

#[test]
fn a_damaged_shard_keeps_what_checks_out_and_is_mended() {
    let dir = temp_dir("damaged");
    let mut store = FactsStore::default();
    store.attach(dir.clone(), None);
    store.put(PhotoId(1), facts(1));
    store.put(PhotoId(2), facts(2));
    let shard = dir.join("00.lcf");
    let mut bytes = std::fs::read(&shard).unwrap();
    let first = encode(PhotoId(1), &facts(1)).unwrap().len();
    // the second record cut short, as by a crash while it was written
    bytes.truncate(first + 20);
    std::fs::write(&shard, &bytes).unwrap();

    let mut again = FactsStore::default();
    again.attach(dir.clone(), None);
    assert_eq!(again.get(PhotoId(1), 1).as_deref(), Some(&facts(1)));
    assert_eq!(again.get(PhotoId(2), 2), None);
    assert_eq!(std::fs::metadata(&shard).unwrap().len(), first as u64, "the cut-short tail is gone");
    again.put(PhotoId(2), facts(2));

    let mut third = FactsStore::default();
    third.attach(dir.clone(), None);
    assert_eq!(third.get(PhotoId(2), 2).as_deref(), Some(&facts(2)));

    // garbage throughout: nothing read, nothing broken
    std::fs::write(&shard, vec![0xA5; 5000]).unwrap();
    let mut fourth = FactsStore::default();
    fourth.attach(dir.clone(), None);
    assert_eq!(fourth.get(PhotoId(1), 1), None);
    fourth.put(PhotoId(1), facts(1));
    let mut fifth = FactsStore::default();
    fifth.attach(dir, None);
    assert_eq!(fifth.get(PhotoId(1), 1).as_deref(), Some(&facts(1)));
}

#[test]
fn a_shard_mostly_of_replaced_records_is_rewritten() {
    let dir = temp_dir("compact");
    let mut store = FactsStore::default();
    store.attach(dir.clone(), None);
    for key in 0..40 {
        store.put(PhotoId(1), facts(key));
    }
    let shard = dir.join("00.lcf");
    let one = encode(PhotoId(1), &facts(39)).unwrap().len() as u64;
    assert_eq!(std::fs::metadata(&shard).unwrap().len(), 40 * one);
    let mut again = FactsStore::default();
    again.attach(dir, None);
    assert_eq!(again.get(PhotoId(1), 39).as_deref(), Some(&facts(39)));
    assert_eq!(std::fs::metadata(&shard).unwrap().len(), one);
}

#[test]
fn nothing_is_written_while_the_gate_is_shut() {
    let dir = temp_dir("gate");
    let mut store = FactsStore::default();
    store.attach(dir.clone(), Some(Arc::new(|| false)));
    store.put(PhotoId(1), facts(1));
    assert!(!dir.join("00.lcf").exists());
    // (this session still has it)
    assert_eq!(store.peek(PhotoId(1)).map(|f| f.key), Some(1));
}

#[test]
fn the_key_follows_the_file_the_profile_the_hot_pixels_and_the_build() {
    let t = std::time::Duration::new(1_700_000_000, 5);
    let base = key_of(1, "/a.CR3", 100, t, Some("ab"), None, false);
    assert_eq!(base, key_of(1, "/a.CR3", 100, t, Some("ab"), None, false));
    for other in [
        key_of(2, "/a.CR3", 100, t, Some("ab"), None, false),
        key_of(1, "/b.CR3", 100, t, Some("ab"), None, false),
        key_of(1, "/a.CR3", 101, t, Some("ab"), None, false),
        key_of(1, "/a.CR3", 100, std::time::Duration::new(1_700_000_000, 6), Some("ab"), None, false),
        key_of(1, "/a.CR3", 100, t, Some("cd"), None, false),
        key_of(1, "/a.CR3", 100, t, Some("ab"), Some("dcp:x#1"), false),
        key_of(1, "/a.CR3", 100, t, Some("ab"), None, true),
    ] {
        assert_ne!(base, other);
    }
    assert!(key("/no/such/file.CR3", None, None, false).is_none());
}

/// A raw decoded in one session is not decoded again in the next for what a frontend that develops raws itself asks of
/// it (card 522: about 0.1 s on the engine thread for each photo paged to, every session). A changed file, a camera look
/// the decode found, or a damaged store, are decoded again.
#[test]
fn a_later_session_answers_from_the_kept_facts_without_decoding() {
    let dir = temp_dir("session");
    let (lib, file) = (dir.join("lib"), dir.join("a.CR3"));
    std::fs::write(&file, b"not really a raw: the loader below stands in for the decoder").unwrap();
    let id = PhotoId(1);
    let decodes = Arc::new(AtomicUsize::new(0));
    let with_match = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let session = || {
        let mut s = crate::Session::new();
        s.open_library(&lib, false).unwrap();
        if s.catalog.photo(id).is_none() {
            let mut photo = Photo::new(id, Source::File { path: file.to_string_lossy().into() }, "a.CR3", "CR3", 6000, 4000, "");
            photo.kind = lightcraft_catalog::MediaKind::Raw;
            s.apply_system(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        }
        let (n, matched) = (decodes.clone(), with_match.clone());
        s.media.file_loader = Some(Arc::new(move |_, _, _| {
            n.fetch_add(1, Ordering::SeqCst);
            let camera_match = matched.load(Ordering::SeqCst).then(|| {
                Arc::new(lightcraft_pipeline::CameraMatch {
                    start_hue_sat: None,
                    matrix: lightcraft_color::Mat3::IDENTITY,
                    hue_sat: None,
                    tone: None,
                })
            });
            let info =
                SourceInfo { raw: true, as_shot_temp: 4800.0, camera_tone: Some(tone()), lens: Some(lens()), camera_match, ..Default::default() };
            let mut picture = lightcraft_raster::Rgb32f::new(96, 64);
            for (i, p) in picture.data.iter_mut().enumerate() {
                let v = (i % 97) as f32 / 97.0;
                *p = [v, v * 0.8, v * 0.6];
            }
            Ok((picture, info))
        }));
        s
    };
    // what the Apple renderer asks on every open (`photo.cameraMatch` only for the Camera Matching profile)
    let open = |s: &mut crate::Session, commands: &[&str]| -> Vec<Value> {
        let p = json!({"id": id.0});
        let mut out: Vec<_> = commands.iter().map(|c| s.execute(c, &p).unwrap()).collect();
        let mut lens = s.execute("photo.lensProfile", &json!({"id": id.0, "decode": true})).unwrap();
        // (whether the photo's own record has the correction yet is not what this is about)
        lens["enabled"] = Value::Null;
        out.push(lens);
        // (what the decode recorded, the as-shot white and the lens, goes to disk with the library)
        s.persist().unwrap();
        out
    };
    let all = ["photo.cameraTone", "photo.localTone", "photo.cameraMatch", "photo.mattes"];

    let first = {
        let mut s = session();
        let first = open(&mut s, &all);
        assert_eq!(decodes.load(Ordering::SeqCst), 1);
        assert!(first[0]["tone"].is_object() && first[0]["level"].is_array() && first[0]["colour"].is_object(), "{}", first[0]);
        assert!(first[4]["correction"]["vignette"].is_object(), "{}", first[4]);
        first
    };
    {
        let mut s = session();
        assert_eq!(open(&mut s, &all), first, "the same answers");
        assert_eq!(decodes.load(Ordering::SeqCst), 1, "nothing decoded");
        assert_eq!(s.embedded_lens(id), Some(lens()));
    }

    // the file changed: decoded again, and kept anew
    std::fs::write(&file, b"another file, another length").unwrap();
    {
        let mut s = session();
        open(&mut s, &all);
        assert_eq!(decodes.load(Ordering::SeqCst), 2);
    }
    {
        let mut s = session();
        open(&mut s, &all);
        assert_eq!(decodes.load(Ordering::SeqCst), 2);
    }

    // a camera look is not kept: asking for it decodes, the rest still does not
    std::fs::write(&file, b"a third file, with a camera look in it").unwrap();
    with_match.store(true, Ordering::SeqCst);
    {
        let mut s = session();
        open(&mut s, &all);
        assert_eq!(decodes.load(Ordering::SeqCst), 3);
    }
    {
        let mut s = session();
        open(&mut s, &["photo.cameraTone", "photo.localTone", "photo.mattes"]);
        assert_eq!(decodes.load(Ordering::SeqCst), 3);
        let m = s.execute("photo.cameraMatch", &json!({"id": id.0})).unwrap();
        assert!(m["match"].is_object(), "{m}");
        assert_eq!(decodes.load(Ordering::SeqCst), 4);
    }

    // a damaged store: decoded again, never an error
    for shard in std::fs::read_dir(lib.join(FOLDER)).unwrap() {
        std::fs::write(shard.unwrap().path(), b"\xff\xff\xff\x7f garbage").unwrap();
    }
    {
        let mut s = session();
        open(&mut s, &["photo.cameraTone"]);
        assert_eq!(decodes.load(Ordering::SeqCst), 5);
    }
}
