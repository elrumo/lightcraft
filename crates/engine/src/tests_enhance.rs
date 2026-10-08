//! AI Super Resolution through the command layer: without the model every request is a clear
//! error that comes back at once; with a (synthetic) model the photo is enlarged, written next to
//! the original under a free name, imported with neutral settings and stacked on the original.
//! No test touches the internet: downloads go to a local server.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::Session;
use crate::enhance::{Enhancer, SUPER_RES_MODEL};

pub(crate) fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-enhance-engine-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A `w` × `h` gradient photo on disk, imported; its id.
fn photo(s: &mut Session, dir: &Path, name: &str, (w, h): (usize, usize)) -> u64 {
    let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 255 / w) as u8, (i / w * 255 / h) as u8, 90]).collect();
    let png = lightcraft_codecs::encode_png(
        &lightcraft_codecs::EncodeImage::new(w as u32, h as u32, 3, lightcraft_codecs::Samples::U8(&rgb)),
        &lightcraft_codecs::EncodeMeta::default(),
    )
    .unwrap();
    let path = dir.join(name);
    std::fs::write(&path, png).unwrap();
    let r = s.execute("library.import", &json!({"paths": [path]})).unwrap();
    r["imported"][0].as_u64().unwrap()
}

#[test]
fn without_the_model_the_commands_say_so() {
    let dir = tmp("nomodel");
    let mut s = Session::new().with_fs();
    let id = photo(&mut s, &dir, "a.png", (60, 40));
    let st = s.execute("enhance.model.status", &json!({})).unwrap();
    assert_eq!(st["available"], Enhancer::AVAILABLE);
    assert_eq!(st["ready"], false);
    assert_eq!(st["download"]["status"]["running"], false);
    // downloading needs the user's consent, and names the size, the licence and the author
    let e = s.execute("enhance.model.download", &json!({})).unwrap_err().to_string();
    if Enhancer::AVAILABLE {
        assert!(e.contains("acknowledged") && e.contains("CC-BY-4.0") && e.contains("Hofmann") && e.contains("MB"), "{e}");
        let e = s.execute("enhance.model.download", &json!({"acknowledged": "yes"})).unwrap_err().to_string();
        assert!(e.contains("acknowledged"), "{e}");
    } else {
        assert!(e.contains("not available in this build"), "{e}");
    }
    assert_eq!(s.execute("enhance.model.cancel", &json!({})).unwrap()["cancelled"], false);

    // Super Resolution: an error that says what to do, at once, and nothing was added
    let e = s.execute("enhance.superRes", &json!({"id": id})).unwrap_err().to_string();
    assert!(e.contains("not available in this build") || e.contains("no folder"), "{e}");
    if Enhancer::AVAILABLE {
        s.enhancer.dir = Some(dir.join("models"));
        let e = s.execute("enhance.superRes", &json!({"id": id})).unwrap_err().to_string();
        assert!(e.contains("not installed") && e.contains(SUPER_RES_MODEL), "{e}");
        let e = s.execute("enhance.model.download", &json!({"id": "nope", "acknowledged": true})).unwrap_err().to_string();
        assert!(e.contains("no enhancement model"), "{e}");
    }
    assert_eq!(s.catalog.photos().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A session with a photo and a (synthetic) model in place.
#[cfg(feature = "enhance")]
pub(crate) fn ready(tag: &str, size: (usize, usize)) -> (Session, PathBuf, u64) {
    let dir = tmp(tag);
    let mut s = Session::new().with_fs();
    let id = photo(&mut s, &dir, "IMG_1.png", size);
    s.enhancer.dir = Some(dir.join("models"));
    let file = s.enhancer.model_file(SUPER_RES_MODEL).unwrap();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    lightcraft_enhance::testing::write_synthetic_model(&file, 8, 2).unwrap();
    (s, dir, id)
}

#[cfg(feature = "enhance")]
#[test]
fn super_resolution_enlarges_stacks_and_never_overwrites() {
    let (mut s, dir, id) = ready("flow", (60, 40));
    assert_eq!(s.execute("enhance.model.status", &json!({})).unwrap()["ready"], true);
    let r = s.execute("enhance.superRes", &json!({"id": id})).unwrap();
    let path = r["path"].as_str().unwrap();
    assert!(path.ends_with("IMG_1-SR.tif"), "{path}");
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(120), Some(80)));
    let new = lightcraft_catalog::PhotoId(r["id"].as_u64().unwrap());
    let p = s.catalog.photo(new).unwrap().clone();
    assert_eq!((p.width, p.height), (120, 80));
    // neutral settings: its pixels carry the edits already
    let d = s.develop_of(new).unwrap();
    assert!(d.light.exposure == 0.0 && d.light.contrast == 0.0 && d.light.highlights == 0.0);
    // stacked on the original, expanded, and selected
    let st = s.catalog.stack_of(new).expect("stacked").clone();
    assert_eq!((st.top(), st.photos.len(), st.collapsed), (new, 2, false));
    assert!(st.photos.contains(&lightcraft_catalog::PhotoId(id)));
    assert_eq!(s.selection.active, Some(new));
    // the original is untouched
    assert_eq!(s.catalog.photo(lightcraft_catalog::PhotoId(id)).map(|p| (p.width, p.height)), Some((60, 40)));
    // a second run takes a free name (the enlargement is a new photo, never a replaced file)
    let pid = lightcraft_catalog::PhotoId(id);
    let mut edited = s.develop_of(pid).unwrap().as_ref().clone();
    edited.light.exposure = 1.0;
    s.set_develop(pid, edited, "test").unwrap();
    let r2 = s.execute("enhance.superRes", &json!({"id": id})).unwrap();
    assert!(r2["path"].as_str().unwrap().ends_with("IMG_1-SR-2.tif"), "{r2}");
    assert!(Path::new(path).exists());
    // the result renders
    assert_eq!(s.render_now(new, 64, 64).unwrap().image.width.max(s.render_now(new, 64, 64).unwrap().image.height), 64);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "enhance")]
#[test]
fn limits_cancels_and_damaged_models_leave_nothing_behind() {
    let (mut s, dir, id) = ready("fail", (60, 40));
    let sr_files = |d: &Path| std::fs::read_dir(d).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains("-SR")).count();
    // too large for the limit: refused before anything is rendered
    let e = s.execute("enhance.superRes", &json!({"id": id, "maxMegapixels": 0.001})).unwrap_err().to_string();
    assert!(e.contains("megapixels"), "{e}");
    assert!(s.execute("enhance.superRes", &json!({"id": id, "maxMegapixels": -3})).is_err());
    assert!(s.execute("enhance.superRes", &json!({"id": id, "maxMegapixels": "lots"})).is_err());
    // the limit is only for that request
    assert!(s.enhancer.max_output_pixels.is_none());
    // cancelled while enlarging: an error, no file, no photo
    let job = s.super_res_job(lightcraft_catalog::PhotoId(id), None).unwrap();
    let e = job.run(&|p, _| p < 0.5).unwrap_err();
    assert!(e.contains("cancelled"), "{e}");
    assert_eq!(sr_files(&dir), 0);
    assert_eq!(s.catalog.photos().count(), 1);
    // a damaged model: an error that names the file, nothing written
    let file = s.enhancer.model_file(SUPER_RES_MODEL).unwrap();
    std::fs::write(&file, b"\x10\x00\x00\x00\x00\x00\x00\x00{not json at all}").unwrap();
    let e = s.execute("enhance.superRes", &json!({"id": id})).unwrap_err().to_string();
    assert!(e.contains("2xNomosUni_span_multijpg.safetensors"), "{e}");
    assert_eq!((sr_files(&dir), s.catalog.photos().count()), (0, 1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "enhance")]
#[test]
fn a_failing_download_ends_with_an_error_and_never_blocks() {
    use std::io::{BufRead, BufReader, Write};
    use std::time::{Duration, Instant};
    // a local mirror that has nothing
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/nomos", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut c in l.incoming().flatten() {
            let mut r = BufReader::new(c.try_clone().unwrap());
            let mut line = String::new();
            while r.read_line(&mut line).unwrap_or(0) > 2 {
                line.clear();
            }
            let _ = c.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 13\r\nConnection: close\r\n\r\n<h1>404</h1>\n");
        }
    });
    let dir = tmp("download404");
    std::fs::write(dir.join(format!("{SUPER_RES_MODEL}-mirrors.txt")), format!("# test mirror\n{base}\n")).unwrap();
    let mut s = Session::new().with_fs();
    s.enhancer.dir = Some(dir.clone());
    s.enhancer.no_builtin_mirrors = true;
    let t = Instant::now();
    let r = s.execute("enhance.model.download", &json!({"acknowledged": true})).unwrap();
    assert_eq!(r["started"], true);
    assert!(t.elapsed() < Duration::from_secs(1), "starting a download returns at once");
    let t = Instant::now();
    while s.enhancer.download_status().1.running {
        assert!(t.elapsed() < Duration::from_secs(30), "the download never ended");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (id, st) = s.enhancer.download_status();
    assert_eq!(id.as_deref(), Some(SUPER_RES_MODEL));
    assert!(!st.finished && st.error.as_deref().unwrap_or("").contains("not found"), "{st:?}");
    assert!(!s.enhancer.installed(SUPER_RES_MODEL));
    // nothing under a real name, and no error page saved
    assert!(!dir.join(SUPER_RES_MODEL).join("2xNomosUni_span_multijpg.safetensors").exists());
    // and with no mirror at all, a clear error and nothing started
    std::fs::remove_file(dir.join(format!("{SUPER_RES_MODEL}-mirrors.txt"))).unwrap();
    let e = s.execute("enhance.model.download", &json!({"acknowledged": true})).unwrap_err().to_string();
    assert!(e.contains("LIGHTCRAFT_NOMOS_SPAN_MIRRORS"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An enlargement identical to one in the library (same bytes, another path) is dropped and the
/// existing photo selected; a photo already imported from that path keeps its file.
#[cfg(feature = "enhance")]
#[test]
fn an_identical_enlargement_is_not_added_twice_and_a_photos_own_file_is_never_removed() {
    let (mut s, dir, id) = ready("dup", (60, 40));
    let pid = lightcraft_catalog::PhotoId(id);
    let done = s.super_res_job(pid, None).unwrap().run(&|_, _| true).unwrap();
    let first = s.finish_super_res(done.clone()).unwrap();
    let new = first["id"].as_u64().unwrap();
    // the same bytes under another name: not added, removed, the first one selected
    let copy = dir.join("IMG_1-SR-copy.tif");
    std::fs::copy(&done.path, &copy).unwrap();
    let r = s.finish_super_res(crate::enhance::SuperResDone { path: copy.to_string_lossy().to_string(), ..done.clone() }).unwrap();
    assert_eq!((r["duplicate"].as_bool(), r["id"].as_u64()), (Some(true), Some(new)));
    assert!(!copy.exists() && s.catalog.photos().count() == 2);
    // the file the photo is imported from, offered again: still a duplicate, but its file stays
    let again = s.finish_super_res(done.clone()).unwrap();
    assert_eq!((again["duplicate"].as_bool(), again["id"].as_u64()), (Some(true), Some(new)));
    assert!(std::path::Path::new(&done.path).exists(), "the photo's own file was removed");
    assert_eq!(s.catalog.photos().count(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
