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

#[test]
fn a_clips_length_is_in_its_summary_and_its_source_is_its_file() {
    let (mut s, clip, still, dir) = with_clip("source");
    let q = s.execute("catalog.query", &json!({"filter": {}})).unwrap();
    let of = |id: PhotoId| q["photos"].as_array().unwrap().iter().find(|p| p["id"] == json!(id.0)).unwrap().clone();
    assert!(of(clip)["duration"].as_f64().is_some_and(|d| (d - 2.0).abs() < 0.01), "{q}");
    assert!(of(still)["duration"].is_null());
    let r = s.execute("video.source", &json!({"id": clip.0})).unwrap();
    assert_eq!((r["available"].as_bool(), r["remote"].as_bool()), (Some(true), Some(false)), "{r}");
    assert!(r["trimStart"].is_null() && r["trimEnd"].is_null() && r["cover"].is_null(), "{r}");
    s.execute("video.setTrim", &json!({"id": clip.0, "start": 0.5})).unwrap();
    s.execute("video.setCover", &json!({"id": clip.0, "time": 1.0})).unwrap();
    let t = s.execute("video.source", &json!({"id": clip.0})).unwrap();
    assert_eq!((t["trimStart"].as_f64(), t["trimEnd"].is_null(), t["cover"].as_f64()), (Some(0.5), true, Some(1.0)), "{t}");
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(r["path"], json!(dir.join("IMG_1.MP4").to_string_lossy()), "{r}");
    assert!(s.execute("video.source", &json!({"id": still.0})).is_err());
    // the file gone: nothing to play
    std::fs::remove_file(dir.join("IMG_1.MP4")).unwrap();
    let r = s.execute("video.source", &json!({"id": clip.0})).unwrap();
    assert_eq!((r["available"].as_bool(), r["path"].is_null()), (Some(false), true), "{r}");
}

fn mean(img: &lightcraft_raster::Rgba8) -> f64 {
    img.data.iter().map(|p| f64::from(p[0])).sum::<f64>() / img.data.len() as f64
}

#[test]
fn a_clip_is_trimmed_in_one_undo_step() {
    let (mut s, clip, still, _) = with_clip("trim");
    let r = s.execute("video.setTrim", &json!({"id": clip.0, "start": 0.5, "end": 1.5})).unwrap();
    assert_eq!((r["start"].as_f64(), r["end"].as_f64()), (Some(0.5), Some(1.5)), "{r}");
    assert!(r["length"].as_f64().is_some_and(|l| (l - 1.0).abs() < 1e-9), "{r}");
    let i = s.execute("video.info", &json!({"id": clip.0})).unwrap();
    assert_eq!((i["trimStart"].as_f64(), i["trimEnd"].as_f64(), i["cover"].is_null()), (Some(0.5), Some(1.5), true), "{i}");
    assert!(s.catalog.photo(clip).unwrap().is_edited());
    assert_eq!(s.catalog.photo(clip).unwrap().history.last().map(|h| h.label.as_str()), Some("Trim Video"));
    // one end at a time: a key left out stays, null puts that end back
    s.execute("video.setTrim", &json!({"id": clip.0, "end": null})).unwrap();
    let i = s.execute("video.info", &json!({"id": clip.0})).unwrap();
    assert_eq!((i["trimStart"].as_f64(), i["trimEnd"].is_null()), (Some(0.5), true), "{i}");
    s.execute("video.setTrim", &json!({"id": clip.0, "start": null})).unwrap();
    assert!(s.develop_of(clip).unwrap().video.is_none(), "an untrimmed clip has no video edit");
    // three trims, three undo steps
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.develop_of(clip).unwrap().video.and_then(|v| v.trim_end), Some(1.5));
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.develop_of(clip).unwrap().video.is_none());
    // refused
    for bad in [
        json!({"id": clip.0}),
        json!({"id": clip.0, "start": -1.0}),
        json!({"id": clip.0, "end": 5.0}),
        json!({"id": clip.0, "start": 1.0, "end": 1.0}),
        json!({"id": clip.0, "start": 1.95}),
        json!({"id": clip.0, "start": "soon"}),
        json!({"id": still.0, "start": 0.5}),
    ] {
        assert!(s.execute("video.setTrim", &bad).is_err(), "{bad}");
    }
    assert!(s.develop_of(clip).unwrap().video.is_none());
}

#[test]
fn a_poster_frame_chosen_replaces_the_librarys_pick_everywhere() {
    let (mut s, clip, still, _) = with_clip("cover");
    let before = mean(&s.render_job(clip, 64, 64, false, true).unwrap().run().rendered.unwrap().image);
    let key_before = crate::media::content_key(s.catalog.photo(clip).unwrap());
    let r = s.execute("video.setCover", &json!({"id": clip.0, "time": 1.9})).unwrap();
    assert_eq!(r["cover"].as_f64(), Some(1.9), "{r}");
    // the clip brightens, so a later frame is a brighter picture; the cache key moved with it
    let after = mean(&s.render_job(clip, 64, 64, false, true).unwrap().run().rendered.unwrap().image);
    assert!(after > before + 30.0, "{before} -> {after}");
    assert_ne!(crate::media::content_key(s.catalog.photo(clip).unwrap()), key_before);
    assert_eq!(s.catalog.photo(clip).unwrap().history.last().map(|h| h.label.as_str()), Some("Set Poster Frame"));
    // it is the clip's own: Copy Settings leaves it behind
    let d = s.develop_of(clip).unwrap();
    assert!(lightcraft_develop::extract_groups(&d, &lightcraft_develop::SettingsGroup::ALL).get("video").is_none());
    // undone, the library's pick is back
    s.execute("edit.undo", &json!({})).unwrap();
    let back = mean(&s.render_job(clip, 64, 64, false, true).unwrap().run().rendered.unwrap().image);
    assert!((back - before).abs() < 1.0, "{before} -> {back}");
    // refused
    for bad in [json!({"id": clip.0}), json!({"id": clip.0, "time": 9.0}), json!({"id": clip.0, "time": -0.1}), json!({"id": still.0, "time": 0.5})] {
        assert!(s.execute("video.setCover", &bad).is_err(), "{bad}");
    }
    s.execute("video.setCover", &json!({"id": clip.0, "time": null})).unwrap();
    assert!(s.develop_of(clip).unwrap().video.is_none());
}

#[test]
fn a_trim_and_a_poster_frame_survive_a_reopen_and_a_peers_older_data() {
    let mut d = lightcraft_develop::DevelopSettings::default();
    // settings without a clip's edits write nothing for them (so every older settings hash stays)
    assert!(!serde_json::to_string(&d).unwrap().contains("video"));
    d.video = Some(lightcraft_develop::VideoEdit { trim_start: Some(1.0), trim_end: None, cover: Some(2.5) });
    let json = serde_json::to_string(&d).unwrap();
    assert!(json.contains(r#""video":{"trim_start":1.0,"cover":2.5}"#), "{json}");
    assert_eq!(serde_json::from_str::<lightcraft_develop::DevelopSettings>(&json).unwrap(), d);
    // older data has no such field
    let older = serde_json::to_string(&lightcraft_develop::DevelopSettings::default()).unwrap();
    assert_eq!(serde_json::from_str::<lightcraft_develop::DevelopSettings>(&older).unwrap().video, None);
}

fn clip_mean_red(path: &std::path::Path, t: f64) -> f32 {
    let c = lightcraft_video::Clip::open_path(path).unwrap();
    let img = c.frame_at(t, 64).unwrap();
    img.data.iter().map(|p| p[0]).sum::<f32>() / img.data.len() as f32
}

#[test]
fn an_edited_trimmed_clip_is_written_again_with_both() {
    let (mut s, clip, still, dir) = with_clip("export");
    s.execute("video.setTrim", &json!({"id": clip.0, "start": 0.5, "end": 1.5})).unwrap();
    let plain = dir.join("plain.mp4");
    let r = s.execute("video.export", &json!({"id": clip.0, "path": plain.to_string_lossy(), "edits": false, "wait": true})).unwrap();
    assert_eq!(
        (r["frames"].as_u64(), r["width"].as_u64(), r["height"].as_u64(), r["audio"].as_bool()),
        (Some(10), Some(128), Some(96), Some(false)),
        "{r}"
    );
    assert!(r["seconds"].as_f64().is_some_and(|d| (d - 1.0).abs() < 1e-9), "{r}");
    let info = lightcraft_video::probe_path(&plain).unwrap();
    assert_eq!(info.frames, 10, "{info:?}");
    assert!((info.duration - 1.0).abs() < 0.06, "{}", info.duration);
    // the part kept starts at the 5th picture of the original
    let original = lightcraft_video::Clip::open_path(&dir.join("IMG_1.MP4")).unwrap();
    let want = original.frame_at(0.5, 64).unwrap();
    let want = want.data.iter().map(|p| p[0]).sum::<f32>() / want.data.len() as f32;
    let got = clip_mean_red(&plain, 0.0);
    assert!((got - want).abs() < want * 0.2, "{got} vs {want}");
    // no half-written file is left
    assert!(!dir.join("plain.mp4.part").exists());
    // the develop settings are applied to every picture
    s.selection = crate::Selection::single(clip);
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    let bright = dir.join("bright.mp4");
    s.execute("video.export", &json!({"id": clip.0, "path": bright.to_string_lossy(), "wait": true})).unwrap();
    assert!(clip_mean_red(&bright, 0.5) > clip_mean_red(&plain, 0.5) * 1.3, "{} vs {}", clip_mean_red(&bright, 0.5), clip_mean_red(&plain, 0.5));
    // and the whole clip comes out when trim is off, smaller when asked
    let whole = dir.join("whole.mp4");
    let r = s.execute("video.export", &json!({"id": clip.0, "path": whole.to_string_lossy(), "trim": false, "maxEdge": 64, "wait": true})).unwrap();
    assert_eq!((r["frames"].as_u64(), r["width"].as_u64(), r["height"].as_u64()), (Some(20), Some(64), Some(48)), "{r}");
    // the status says how it went
    let st = s.execute("video.exportStatus", &json!({})).unwrap();
    assert_eq!((st["running"].as_bool(), st["result"]["frames"].as_u64()), (Some(false), Some(20)), "{st}");
    // refused: not an mp4, not a clip, a bad size or quality, an original
    for bad in [
        json!({"id": clip.0, "path": dir.join("x.mov").to_string_lossy()}),
        json!({"id": still.0, "path": dir.join("y.mp4").to_string_lossy()}),
        json!({"id": clip.0, "path": dir.join("z.mp4").to_string_lossy(), "maxEdge": 8}),
        json!({"id": clip.0, "path": dir.join("z.mp4").to_string_lossy(), "quality": "tiny"}),
        json!({"id": clip.0}),
        json!({"id": clip.0, "path": dir.join("IMG_1.MP4").to_string_lossy()}),
    ] {
        assert!(s.execute("video.export", &bad).is_err(), "{bad}");
    }
    assert!(dir.join("IMG_1.MP4").metadata().unwrap().len() > 100, "the original is untouched");
}

#[test]
fn a_cancelled_export_leaves_nothing() {
    let (mut s, clip, _, dir) = with_clip("cancel");
    let target = dir.join("never.mp4");
    let prepared = crate::video_export::prepare(&mut s, &json!({"id": clip.0, "path": target.to_string_lossy()})).unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let r = prepared.run(&|_, _| {}, &cancel);
    assert!(r.as_ref().is_err_and(|e| e.contains("cancelled")), "{r:?}");
    assert!(!target.exists() && !dir.join("never.mp4.part").exists());
}

#[test]
fn a_frame_can_be_asked_for_with_the_edits() {
    use base64::Engine as _;
    let (mut s, clip, _, _) = with_clip("frame-edits");
    let red = |r: &serde_json::Value| {
        let bytes = base64::engine::general_purpose::STANDARD.decode(r["png"].as_str().unwrap()).unwrap();
        mean(&lightcraft_codecs::decode(&bytes, Default::default()).unwrap().to_srgb8())
    };
    s.selection = crate::Selection::single(clip);
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    let plain = red(&s.execute("video.frame", &json!({"id": clip.0, "time": 1.0})).unwrap());
    let edited = red(&s.execute("video.frame", &json!({"id": clip.0, "time": 1.0, "edits": true})).unwrap());
    assert!(edited > plain * 1.2, "{plain} vs {edited}");
    // with no time, the poster frame the clip was given
    s.execute("video.setCover", &json!({"id": clip.0, "time": 1.9})).unwrap();
    let cover = red(&s.execute("video.frame", &json!({"id": clip.0})).unwrap());
    assert!(cover > red(&s.execute("video.frame", &json!({"id": clip.0, "time": 0.0})).unwrap()) + 20.0);
}
