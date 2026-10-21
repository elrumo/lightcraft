//! Video clips in the library: import, the Videos filter, and the poster frame that stands for a clip.

use std::path::PathBuf;

use lightcraft_catalog::{MediaKind, PhotoId};
use serde_json::json;

use crate::Session;

fn folder(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-video-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_clip_imports_as_a_video_with_its_length_and_a_poster() {
    let dir = folder("import");
    let bytes = lightcraft_video::testdata::clip(20, 10, 2);
    assert!(!bytes.is_empty());
    std::fs::write(dir.join("IMG_0001.MP4"), &bytes).unwrap();
    // (a stray non-media file is left alone)
    std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(1), "{r}");
    let id = PhotoId(r["imported"][0].as_u64().unwrap());
    let p = s.catalog.photo(id).unwrap().clone();
    assert_eq!(p.kind, MediaKind::Video);
    assert_eq!((p.width, p.height), (128, 96));
    assert_eq!(p.format, "MP4");
    assert_eq!(p.file_size, bytes.len() as u64);
    assert!(p.duration.is_some_and(|d| (d - 2.0).abs() < 0.01), "{:?}", p.duration);
    // the hash is that of the whole file, however it was read (the copy check and sync dedupe rely on it)
    assert_eq!(p.content_hash.as_deref(), Some(lightcraft_preview::hash_bytes(&bytes).to_string().as_str()));
    // it is in the Videos filter and not in Photos
    s.execute("library.filter", &json!({"kind": "video"})).unwrap();
    assert_eq!(s.visible(), &[id]);
    s.execute("library.filter", &json!({"kind": "image"})).unwrap();
    assert!(s.visible().is_empty());
    s.execute("library.filter", &json!({})).unwrap();
    // its poster renders like any picture
    assert!(s.render_job(id, 64, 64, false, true).unwrap().run().rendered.is_ok());
}

#[test]
fn a_clip_probed_from_its_bytes_matches_the_one_probed_from_its_file() {
    let dir = folder("probe");
    let bytes = lightcraft_video::testdata::clip(10, 5, 0);
    let path = dir.join("a.mov");
    std::fs::write(&path, &bytes).unwrap();
    let from_bytes = crate::files::probe_bytes("a.mov", &bytes).unwrap();
    let (_, probe) = crate::files::fs_hooks();
    let from_file = probe(&path.to_string_lossy()).unwrap();
    assert_eq!((from_bytes.kind, from_file.kind), (MediaKind::Video, MediaKind::Video));
    assert_eq!((from_bytes.width, from_bytes.height, from_bytes.duration), (from_file.width, from_file.height, from_file.duration));
    assert_eq!((from_bytes.content_hash, from_bytes.file_size), (from_file.content_hash, from_file.file_size));
    assert_eq!(from_file.format, "MOV");
}

#[test]
fn a_damaged_clip_fails_the_import_with_a_reason_and_the_rest_goes_through() {
    let dir = folder("damaged");
    let good = lightcraft_video::testdata::clip(10, 5, 0);
    std::fs::write(dir.join("good.mp4"), &good).unwrap();
    std::fs::write(dir.join("cut.mp4"), &good[..good.len() / 3]).unwrap();
    std::fs::write(dir.join("junk.mov"), vec![0xA5u8; 4096]).unwrap();
    std::fs::write(dir.join("empty.mp4"), b"").unwrap();
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(1), "{r}");
    assert!(r["failed"].as_array().is_some_and(|f| f.len() == 3), "{r}");
    assert_eq!(s.catalog.len(), 1);
}

/// A session holding the clip `IMG_1.MP4` (grey that brightens picture by picture) and a picture `still.png`.
fn with_clip(tag: &str) -> (Session, PhotoId, PhotoId, PathBuf) {
    let dir = folder(tag);
    std::fs::write(dir.join("IMG_1.MP4"), lightcraft_video::testdata::clip(20, 10, 2)).unwrap();
    let img = lightcraft_raster::Rgba8::from_fn(16, 12, |x, y| [(x * 9) as u8, (y * 11) as u8, 7, 255]);
    let png =
        crate::export::encode_image(&img, &crate::export::ExportOptions { format: crate::export::ExportFormat::Png, ..Default::default() }).unwrap();
    std::fs::write(dir.join("still.png"), png).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let find = |s: &Session, kind| s.catalog.photos().find(|p| p.kind == kind).map(|p| p.id).unwrap();
    let (clip, still) = (find(&s, MediaKind::Video), find(&s, MediaKind::Image));
    (s, clip, still, dir)
}

#[test]
fn video_info_reports_the_clip_and_refuses_a_picture() {
    let (mut s, clip, still, _) = with_clip("info");
    let i = s.execute("video.info", &json!({"id": clip.0})).unwrap();
    assert_eq!((i["width"].as_u64(), i["height"].as_u64(), i["codec"].as_str()), (Some(128), Some(96), Some("h264")), "{i}");
    assert!(i["duration"].as_f64().is_some_and(|d| (d - 2.0).abs() < 0.01), "{i}");
    assert_eq!((i["hasAudio"].as_bool(), i["hdr"].as_bool(), i["unsupported"].is_null()), (Some(false), Some(false), true), "{i}");
    let e = s.execute("video.info", &json!({"id": still.0})).unwrap_err().to_string();
    assert!(e.contains("not a video"), "{e}");
    assert!(s.execute("video.info", &json!({"id": 9999})).is_err());
    // with no `id`, the active photo
    s.selection.ids = vec![clip];
    s.selection.active = Some(clip);
    assert_eq!(s.execute("video.info", &json!({})).unwrap()["photo"], json!(clip.0));
}

#[test]
fn video_frame_returns_the_picture_at_a_time_or_writes_it() {
    use base64::Engine as _;
    let (mut s, clip, _, dir) = with_clip("frame");
    let decode = |r: &serde_json::Value| {
        let bytes = base64::engine::general_purpose::STANDARD.decode(r["png"].as_str().unwrap()).unwrap();
        lightcraft_codecs::decode(&bytes, Default::default()).unwrap().to_srgb8()
    };
    let early = s.execute("video.frame", &json!({"id": clip.0, "time": 0.0})).unwrap();
    let late = s.execute("video.frame", &json!({"id": clip.0, "time": 1.9})).unwrap();
    let (a, b) = (decode(&early), decode(&late));
    assert_eq!((a.width, a.height), (128, 96));
    assert!(b.data[0][0] > a.data[0][0] + 40, "{:?} {:?}", a.data[0], b.data[0]);
    // the poster when no time is given; written to a file when asked
    let path = dir.join("poster.jpg");
    let r = s.execute("video.frame", &json!({"id": clip.0, "maxEdge": 64, "path": path.to_string_lossy()})).unwrap();
    assert_eq!(r["path"], json!(path.to_string_lossy()), "{r}");
    let jpg = lightcraft_codecs::decode(&std::fs::read(&path).unwrap(), Default::default()).unwrap();
    assert!(jpg.width <= 128 && jpg.width >= 64, "{}", jpg.width);
    // bad parameters are errors
    for bad in [json!({"id": clip.0, "maxEdge": 8}), json!({"id": clip.0, "maxEdge": 99999}), json!({"id": clip.0, "time": "soon"})] {
        assert!(s.execute("video.frame", &bad).is_err(), "{bad}");
    }
}
