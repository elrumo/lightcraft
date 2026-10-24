//! Olympus / OM System files in the engine: the `.ori` originals OM Digital Solutions writes for its High Res and
//! Pro Capture modes (an ORF container), and what a preview-only ORF imports with. Synthetic containers; the real
//! files are pinned in `crates/raw/tests/it/olympus_samsung_corpus.rs`.

use std::path::PathBuf;

use lightcraft_catalog::MediaKind;
use lightcraft_tiff::{ByteOrder, IfdBuilder, ImageData, TiffWriter, Value, tags as t};
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

/// A new-style Olympus maker note (`OLYMPUS\0II\x03\0`: the IFD at 12, offsets counted from the note's start) whose
/// CameraSettings directory (0x2020) names `preview` as its PreviewImage (0x0101 start, 0x0102 length).
fn maker_note(preview: &[u8]) -> Vec<u8> {
    let entry = |tag: u16, typ: u16, v: u32| {
        [tag.to_le_bytes().to_vec(), typ.to_le_bytes().to_vec(), 1u32.to_le_bytes().to_vec(), v.to_le_bytes().to_vec()].concat()
    };
    let camera_settings = 12 + 2 + 12 + 4;
    let jpeg_at = camera_settings + 2 + 2 * 12 + 4;
    let mut n = b"OLYMPUS\0II\x03\0".to_vec();
    n.extend(1u16.to_le_bytes());
    n.extend(entry(0x2020, 13, camera_settings as u32));
    n.extend(0u32.to_le_bytes());
    n.extend(2u16.to_le_bytes());
    n.extend(entry(0x0101, 4, jpeg_at as u32));
    n.extend(entry(0x0102, 4, preview.len() as u32));
    n.extend(0u32.to_le_bytes());
    n.extend(preview);
    n
}

/// An Olympus container (the `IIRO` magic) for a `w × h` sensor of `bits`-bit samples whose only strip is `strip`,
/// shot with the camera turned as `orientation` says, with `preview` as its CameraSettings preview and a few Exif
/// fields (ISO 200, 1/125 s, an RGGB colour-filter layout).
fn container(w: u32, h: u32, bits: u16, strip: Vec<u8>, orientation: u16, preview: &[u8]) -> Vec<u8> {
    let mut ifd = IfdBuilder::new();
    ifd.set(t::IMAGE_WIDTH, Value::Long(vec![w]));
    ifd.set(t::IMAGE_LENGTH, Value::Long(vec![h]));
    ifd.set(t::BITS_PER_SAMPLE, Value::Short(vec![bits]));
    ifd.set(t::COMPRESSION, Value::Short(vec![1]));
    ifd.set(t::PHOTOMETRIC, Value::Short(vec![1]));
    ifd.set(t::MAKE, Value::Ascii("OLYMPUS IMAGING CORP.".into()));
    ifd.set(t::MODEL, Value::Ascii("E-M10 Mark II".into()));
    ifd.set(t::ORIENTATION, Value::Short(vec![orientation]));
    ifd.set_image(ImageData::Strips { rows_per_strip: h, strips: vec![strip] });
    let mut exif = IfdBuilder::new();
    exif.set(t::EXPOSURE_TIME, Value::Rational(vec![(1, 125)]));
    exif.set(t::ISO_SPEED, Value::Short(vec![200]));
    exif.set(0xa302, Value::Undefined(vec![0, 2, 0, 2, 0, 1, 1, 2]));
    exif.set(t::MAKER_NOTE, Value::Undefined(maker_note(preview)));
    ifd.set_child(t::EXIF_IFD, exif);
    let mut b = TiffWriter::new(ByteOrder::Little, false).write(&[ifd]).unwrap();
    b[2] = b'R';
    b[3] = b'O';
    b
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
