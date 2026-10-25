//! OM Digital Solutions' `.ori` files, the originals of its High Res and Pro Capture shots: four CC0 files from raw.pixls.us
//! (URLs and SHA-256 in `RAW_SAMPLES`, xtask/src/main.rs; downloaded into the gitignored corpus/raw by `cargo xtask corpus
//! --download`). The other fifteen ORIs of the archive (OM-1, OM-1 Mark II, OM-3, OM-5, OM-5 Mark II, E-M1 Mark II, E-M1 Mark III)
//! were checked by hand the same way on 2026-10-24.
//!
//! An ORI is an ORF container (`IIRO`) with one single-frame strip in the compressed coding the ORFs of the same bodies use
//! (compression 1, far fewer bytes than the samples need, a strip that starts `00 00 00 00 01 00 00`), the same maker note and
//! a 3200 × 2400 camera JPEG. So it is recognised as an ORF, refused as a compressed ORF (the decoder is on hold: see
//! `crates/raw/src/vendor/orf.rs`) and shown from its JPEG. Missing files are reported as skipped.

use lightcraft_raw::{RawError, RawFormat, decode, embedded_preview, probe, probe_info};
use std::path::{Path, PathBuf};

fn corpus() -> PathBuf {
    std::env::var_os("LIGHTCRAFT_CORPUS").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"))
}

/// Pixel size of a JPEG, from its headers.
fn jpeg_size(jpeg: &[u8]) -> (usize, usize) {
    let mut d = zune_jpeg::JpegDecoder::new(zune_core::bytestream::ZCursor::new(jpeg));
    d.decode_headers().unwrap();
    let info = d.info().unwrap();
    (usize::from(info.width), usize::from(info.height))
}

#[test]
fn ori_files_are_orf_containers_in_the_compressed_coding_with_a_camera_jpeg() {
    // (file, Exif Make, Exif Model, lens)
    let cases = [
        ("ori-olympus-e-m5-mark-ii.ori", "OLYMPUS IMAGING CORP.", "E-M5MarkII", "OLYMPUS M.45mm F1.8"),
        ("ori-olympus-e-m1-mark-iii.ori", "OLYMPUS CORPORATION", "E-M1MarkIII", "OLYMPUS M.12-40mm F2.8"),
        ("ori-om-system-om-1-mark-ii.ori", "OM Digital Solutions", "OM-1MarkII", "OLYMPUS M.12-40mm F2.8"),
        ("ori-om-digital-om-3.ori", "OM Digital Solutions", "OM-3", "OLYMPUS M.17mm F1.2"),
    ];
    let mut seen = 0;
    for (name, make, model, lens) in cases {
        let path = corpus().join("raw").join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            lightcraft_testkit::skip!(Corpus, "{} absent", path.display());
            continue;
        };
        assert_eq!(probe(&bytes), Some(RawFormat::Orf), "{name}: recognised by its content, not its extension");
        // refused as the coding it is, by decode and by the header probe alike
        for e in [decode(&bytes).map(|_| ()), probe_info(&bytes).map(|_| ())] {
            assert!(matches!(&e, Err(RawError::Unsupported(w)) if w == "Olympus compressed ORF"), "{name}: {e:?}");
        }
        // shown from the camera's own JPEG: the only large one in the file
        let jpeg = embedded_preview(&bytes).unwrap_or_else(|| panic!("{name}: no camera JPEG"));
        assert!(jpeg.starts_with(&[0xff, 0xd8]));
        assert_eq!(jpeg_size(&jpeg), (3200, 2400), "{name}");
        let m = lightcraft_meta::extract(&bytes);
        assert_eq!((m.make.as_deref(), m.model.as_deref(), m.lens_model.as_deref()), (Some(make), Some(model), Some(lens)), "{name}");
        assert_eq!(m.orientation, Some(lightcraft_geom::Orientation::Normal), "{name}");
        assert!(m.iso.is_some() && m.exposure_time.is_some() && m.capture_time.is_some(), "{name}: exposure and date");
        seen += 1;
    }
    eprintln!("OM Digital Solutions ORI files checked: {seen}");
}
