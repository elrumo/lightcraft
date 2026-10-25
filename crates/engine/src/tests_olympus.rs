//! Olympus / OM System files in the engine: the `.ori` originals OM Digital Solutions writes for its High Res and
//! Pro Capture modes (an ORF container), and what a preview-only ORF imports with. Synthetic containers; the real
//! files are pinned in `crates/raw/tests/it/olympus_samsung_corpus.rs`.

use std::path::PathBuf;

use lightcraft_catalog::MediaKind;
use lightcraft_raw::testdata::OlympusOrf;
use serde_json::json;

use crate::Session;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-olympus-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A baseline JPEG of `w × h` pixels.
fn jpeg(w: usize, h: usize) -> Vec<u8> {
    use lightcraft_codecs::{ChromaSubsampling, EncodeImage, EncodeMeta, Samples, encode_jpeg};
    let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, 128, 200]).collect();
    encode_jpeg(&EncodeImage::new(w as u32, h as u32, 3, Samples::U8(&px)), 90, ChromaSubsampling::S444, &EncodeMeta::default()).unwrap()
}

/// An Olympus container (the `IIRO` magic) for a `w × h` sensor of `bits`-bit samples whose only strip is `strip`,
/// shot with the camera turned as `orientation` says, with `preview` as its CameraSettings preview and a few Exif
/// fields (ISO 200, 1/125 s, an RGGB colour-filter layout): `lightcraft_raw::testdata::OlympusOrf`.
fn container(w: u32, h: u32, bits: u16, strip: Vec<u8>, orientation: u16, preview: &[u8]) -> Vec<u8> {
    container_framed(w, h, bits, strip, orientation, preview, None)
}

/// [`container`] shot at an aspect ratio other than 4:3: `frame` is the note's `AspectFrame`.
fn container_framed(w: u32, h: u32, bits: u16, strip: Vec<u8>, orientation: u16, preview: &[u8], frame: Option<[u16; 4]>) -> Vec<u8> {
    OlympusOrf { orientation, frame, ..OlympusOrf::new(w, h, bits, strip, preview.to_vec()) }.build()
}

/// A strip of the compressed coding's size class: far shorter than the sensor's samples, so no uncompressed layout fits.
fn compressed_strip(seed: u8) -> Vec<u8> {
    [&[0u8, 0, 0, 0, 1, 0, 0][..], &[seed; 57]].concat()
}

#[test]
fn ori_is_an_import_extension_and_a_raw_format() {
    use crate::import::{EXTENSIONS, RAW_EXTENSIONS, accept_list, is_supported};
    assert!(EXTENSIONS.contains(&"ori") && RAW_EXTENSIONS.contains(&"ori"));
    assert!(is_supported(std::path::Path::new("P5048183.ORI")) && is_supported(std::path::Path::new("/card/P5048183.ori")));
    let accept = accept_list();
    assert!(accept.contains(".ori,") && accept.contains(".ORI"), "the web picker offers them: {accept}");
}

/// A folder holding an ORI and an ORF with the same compressed coding: both import as preview-only raws, shown from the
/// camera's JPEG turned the way the camera was held, with the file's metadata. The ORI says what it is.
#[test]
fn a_compressed_ori_imports_as_a_preview_only_raw_turned_and_described_like_an_orf() {
    let src = temp_dir("ori-compressed");
    // the camera held upright: the container says "rotate 90 degrees clockwise", the JPEG (40 × 30) carries no tag
    std::fs::write(src.join("P5048183.ORI"), container(5220, 3912, 16, compressed_strip(1), 6, &jpeg(40, 30))).unwrap();
    std::fs::write(src.join("P5048184.ORF"), container(5220, 3912, 16, compressed_strip(2), 6, &jpeg(40, 30))).unwrap();
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(r["scanned"], 2, "{r}");
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(2), "{r}");
    let by_name = |name: &str| s.catalog.photos().find(|p| p.file_name == name).unwrap_or_else(|| panic!("{name} not imported")).clone();
    let (ori, orf) = (by_name("P5048183.ORI"), by_name("P5048184.ORF"));
    for p in [&ori, &orf] {
        assert_eq!(p.kind, MediaKind::Raw, "{}", p.file_name);
        assert_eq!((p.width, p.height), (30, 40), "{}: the preview, turned", p.file_name);
        assert!(p.meta.camera.contains("E-M10 Mark II"), "{}: {}", p.file_name, p.meta.camera);
        assert_eq!(p.meta.iso, Some(200), "{}", p.file_name);
        assert!(!p.develops_raw(), "{}: shown from the camera's JPEG", p.file_name);
    }
    assert_eq!(ori.format, "ORI");
    assert_eq!(orf.format, "ORF");
    let why = ori.preview_only.clone().expect("preview only");
    assert!(why.contains("ORI") && why.contains("compressed"), "{why}");
    assert_eq!(orf.preview_only.as_deref(), Some("Olympus compressed ORF"), "an ORF keeps the decoder's wording");
    // the loupe draws it from that JPEG
    let id = ori.id;
    assert!(s.render_job(id, 30, 40, false, true).unwrap().run().rendered.is_ok());
    let _ = std::fs::remove_dir_all(&src);
}

/// An ORI whose strip is one of the uncompressed layouts is developed from its sensor data like an ORF would be: the
/// container, not the extension, decides. It keeps the `ORI` name and, as every Olympus file without colour matrices,
/// edits white balance relative to its as-shot look.
#[test]
fn an_ori_with_an_uncompressed_strip_develops_as_a_raw() {
    let src = temp_dir("ori-uncompressed");
    let (w, h) = (16u32, 8u32);
    let words: Vec<u8> = (0..w * h).flat_map(|i| (((200 + i * 37) % 4000) as u16).to_le_bytes()).collect();
    std::fs::write(src.join("P5048185.ori"), container(w, h, 16, words, 1, &jpeg(40, 30))).unwrap();
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(1), "{r}");
    let p = s.catalog.photos().next().unwrap().clone();
    assert_eq!((p.kind, p.format.as_str(), p.preview_only.as_deref()), (MediaKind::Raw, "ORI", None));
    assert_eq!((p.width, p.height), (16, 8), "the sensor's size, not the preview's");
    assert!(p.develops_raw() && p.relative_wb() && p.fitted_look(), "a raw like an ORF: look fitted to its JPEG, relative white balance");
    assert!(s.render_job(p.id, 16, 8, false, true).unwrap().run().rendered.is_ok());
    let _ = std::fs::remove_dir_all(&src);
}

/// A preview-only raw is described in its preview's frame, the one Lightroom's crop edges in a sidecar refer to: the
/// probe says how it is turned, and a sidecar's crop of the stored image's left half lands on the top half of the
/// portrait that shows it (before, the crop was read in the shown frame: the left half of the portrait).
#[test]
fn a_preview_only_raw_has_its_stored_frame_for_lightroom_crops() {
    use crate::media::ProbeInfo;
    use lightcraft_geom::Orientation;
    let landscape = container(5184, 3888, 16, compressed_strip(1), 1, &jpeg(40, 30));
    let portrait = container(5184, 3888, 16, compressed_strip(2), 6, &jpeg(40, 30));
    let probe = |name: &str, bytes: &[u8]| -> ProbeInfo { crate::files::probe_bytes(name, bytes).unwrap() };
    assert_eq!(probe("a.orf", &landscape).orientation, Some(Orientation::Normal));
    let p = probe("b.orf", &portrait);
    assert_eq!((p.width, p.height, p.orientation), (30, 40, Some(Orientation::Rotate90)));
    assert_eq!(p.stored_frame().map(|f| (f.width, f.height)), Some((40.0, 30.0)));

    let src = temp_dir("stored-frame");
    std::fs::write(src.join("b.orf"), &portrait).unwrap();
    let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:HasCrop="True" crs:CropLeft="0" crs:CropTop="0" crs:CropRight="0.5" crs:CropBottom="1" crs:CropAngle="0"/></rdf:RDF></x:xmpmeta>"#;
    std::fs::write(src.join("b.xmp"), xmp).unwrap();
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(1), "{r}");
    let rect = s.catalog.photos().next().unwrap().develop.crop.geometry.rect;
    let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
    assert!(near(rect.x0, 0.0) && near(rect.x1, 1.0) && near(rect.y0, 0.0) && near(rect.y1, 0.5), "{rect:?}");
    let _ = std::fs::remove_dir_all(&src);

    // a straightened crop is read in the stored frame too: Lightroom turns clockwise, LightCraft's angle is the other way
    let src = temp_dir("stored-frame-angle");
    std::fs::write(src.join("c.orf"), &landscape).unwrap();
    std::fs::write(src.join("c.xmp"), xmp.replace("crs:CropAngle=\"0\"", "crs:CropAngle=\"1.5\"")).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    let angle = s.catalog.photos().next().unwrap().develop.crop.geometry.angle;
    assert!(near(angle, -1.5), "{angle}");
    let _ = std::fs::remove_dir_all(&src);
}

/// A shot at 3:2, 16:9 or 1:1 holds the whole 4:3 sensor area and its preview, and says which frame the camera's own
/// JPEG was cut to. A preview-only photo takes that frame as its crop, as part of its import look (not an edit, Reset
/// keeps it, the crop tool can widen it) and in the shown frame, turned like the picture. A 4:3 shot, and one whose
/// frame doesn't fit its area, get no crop.
#[test]
fn an_aspect_frame_becomes_the_import_crop_of_a_preview_only_raw() {
    let src = temp_dir("aspect-frame");
    // 16:9 of a 5184 x 3888 area: the rows 484 to 3403
    let wide = [0, 484, 5183, 3403];
    let write = |name: &str, orientation: u16, seed: u8, frame: Option<[u16; 4]>| {
        std::fs::write(src.join(name), container_framed(5184, 3888, 16, compressed_strip(seed), orientation, &jpeg(40, 30), frame)).unwrap();
    };
    write("landscape.orf", 1, 1, Some(wide));
    write("portrait.orf", 6, 2, Some(wide));
    write("four-three.orf", 1, 3, Some([0, 0, 5183, 3887]));
    write("damaged.orf", 1, 4, Some([0, 484, 6000, 3403]));
    write("none.orf", 1, 5, None);
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(5), "{r}");
    let photo = |name: &str| s.catalog.photos().find(|p| p.file_name == name).unwrap().clone();
    let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
    let (top, bottom) = (484.0 / 3888.0, 3404.0 / 3888.0);

    let p = photo("landscape.orf");
    let r = p.develop.crop.geometry.rect;
    assert!(near(r.x0, 0.0) && near(r.x1, 1.0) && near(r.y0, top) && near(r.y1, bottom), "{r:?}");
    assert!(!p.is_edited(), "an import look, not an edit");
    assert!(p.develop_is(p.import_defaults()));

    // the camera held upright (stored image turned 90 degrees clockwise): the frame's rows become columns
    let p = photo("portrait.orf");
    let r = p.develop.crop.geometry.rect;
    assert!(near(r.y0, 0.0) && near(r.y1, 1.0) && near(r.x0, 1.0 - bottom) && near(r.x1, 1.0 - top), "{r:?}");
    assert!(!p.is_edited());

    for name in ["four-three.orf", "damaged.orf", "none.orf"] {
        let p = photo(name);
        assert_eq!(p.develop.crop.geometry.rect, lightcraft_geom::Rect::UNIT, "{name}");
        assert!(p.import_look.is_none(), "{name}");
    }
    let _ = std::fs::remove_dir_all(&src);
}

/// A raw that decodes (here an uncompressed 16-bit ORF; the same holds for a decoded ORI) takes the aspect frame of a
/// 16:9 shot as its import crop too: the whole 4:3 area is developed, the camera's own JPEG is cut to the frame.
#[test]
fn an_aspect_frame_becomes_the_import_crop_of_a_decoded_raw() {
    let src = temp_dir("aspect-frame-decoded");
    // a 48 x 36 sensor, all of it the valid area; 16:9 is its rows 4 to 30
    let words: Vec<u8> = (0..48u32 * 36).flat_map(|i| (((200 + i * 37) % 4000) as u16).to_le_bytes()).collect();
    let write = |name: &str, orientation: u16, frame: Option<[u16; 4]>| {
        std::fs::write(src.join(name), container_framed(48, 36, 16, words.clone(), orientation, &jpeg(40, 30), frame)).unwrap();
    };
    write("wide.ori", 1, Some([0, 4, 47, 30]));
    write("upright.orf", 8, Some([0, 4, 47, 30]));
    write("four-three.orf", 1, None);
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(3), "{r}");
    let photo = |name: &str| s.catalog.photos().find(|p| p.file_name == name).unwrap().clone();
    let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
    let (top, bottom) = (4.0 / 36.0, 31.0 / 36.0);

    let p = photo("wide.ori");
    assert!(p.develops_raw() && (p.width, p.height) == (48, 36), "the whole area is developed");
    let r = p.develop.crop.geometry.rect;
    assert!(near(r.x0, 0.0) && near(r.x1, 1.0) && near(r.y0, top) && near(r.y1, bottom), "{r:?}");
    assert!(!p.is_edited());
    let wide_id = p.id;

    // held upright the other way (EXIF 8, a quarter turn anticlockwise): the frame's rows become columns
    let p = photo("upright.orf");
    let r = p.develop.crop.geometry.rect;
    assert!(near(r.y0, 0.0) && near(r.y1, 1.0) && near(r.x0, top) && near(r.x1, bottom), "{r:?}");

    let p = photo("four-three.orf");
    assert_eq!(p.develop.crop.geometry.rect, lightcraft_geom::Rect::UNIT);
    assert!(s.render_job(wide_id, 48, 36, false, true).unwrap().run().rendered.is_ok());
    let _ = std::fs::remove_dir_all(&src);
}

/// A preview-only photo whose raw later decodes (a newer LightCraft reads the file) heals as the decoded raw it is and
/// keeps the aspect crop it was imported with: the healed photo's settings are made afresh with the crop in them. (Its
/// import look is the one it was imported with, as for a photo imported with a default preset: no catalog op sets it, so
/// the photo reads as edited until Reset.)
#[test]
fn a_healed_raw_keeps_its_aspect_crop() {
    let src = temp_dir("aspect-frame-heal");
    let words: Vec<u8> = (0..48u32 * 36).flat_map(|i| (((200 + i * 37) % 4000) as u16).to_le_bytes()).collect();
    let frame = Some([0, 4, 47, 30]);
    let path = src.join("a.orf");
    std::fs::write(&path, container_framed(48, 36, 16, compressed_strip(1), 1, &jpeg(40, 30), frame)).unwrap();
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    let id = lightcraft_catalog::PhotoId(r["imported"][0].as_u64().unwrap());
    assert!(s.catalog.photo(id).unwrap().preview_only.is_some());
    let crop = s.catalog.photo(id).unwrap().develop.crop.geometry.rect;
    assert_ne!(crop, lightcraft_geom::Rect::UNIT);
    // the file now decodes
    std::fs::write(&path, container_framed(48, 36, 16, words, 1, &jpeg(40, 30), frame)).unwrap();
    s.media.forget(id);
    let job = s.loupe_job(id, 48, 36, true).unwrap();
    let r = job.run();
    assert!(r.rendered.is_ok());
    s.accept(&r);
    let p = s.catalog.photo(id).unwrap();
    assert!(p.develops_raw(), "healed");
    assert_eq!(p.develop.crop.geometry.rect, crop, "the frame is still the crop");
    let _ = std::fs::remove_dir_all(&src);
}
