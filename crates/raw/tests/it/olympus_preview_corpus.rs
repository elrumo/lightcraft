//! What a compressed Olympus ORF, which opens as its embedded JPEG, gives the library: five CC0 files from raw.pixls.us
//! (URLs and SHA-256 in `RAW_SAMPLES`, xtask/src/main.rs; downloaded into the gitignored corpus/raw by `cargo xtask
//! corpus --download`), one per thing worth pinning. The other 83 Olympus and OM System files of the archive that were
//! swept on 2026-10-24 (88 files from 70 bodies, from the E-3 to the OM-3, the 19 `.ori` included) agree with these
//! on the point they were checked for: the JPEG `embedded_preview` hands on is the largest complete one in the file.
//!
//! - E-30: held upright (container orientation 8, the JPEG stored unturned), and no Exif LensModel: the lens name is
//!   only in the maker note's Equipment directory.
//! - E-P2: shot at 3:2: the sensor area and the 3200 × 2400 JPEG are 4:3, the maker note states the 3:2 frame; its lens
//!   too is only in Equipment.
//! - E-M10 Mark IV: shot at 16:9, the lens in both places.
//! - TG-6: a compact shot at 3:2, no lens name anywhere.
//! - E-3: the 2007 bodies carry only a 1600 × 1200 JPEG besides their thumbnail, and name the lens by a type code.
//!
//! Missing files are reported as skipped.

use lightcraft_geom::Orientation;
use lightcraft_raw::{RawError, decode, embedded_preview, olympus_aspect_frame};
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

/// What the file is expected to give.
struct Expect {
    file: &'static str,
    model: &'static str,
    lens: Option<&'static str>,
    preview: (usize, usize),
    orientation: Orientation,
    /// The aspect frame as the file states it (left, top, right, bottom) and its valid area (width, height).
    frame: Option<([u32; 4], [u32; 2])>,
}

const FILES: &[Expect] = &[
    Expect {
        file: "orf-olympus-e-30.orf",
        model: "E-30",
        lens: Some("OLYMPUS 14-54mm Lens"),
        preview: (3200, 2400),
        orientation: Orientation::Rotate270,
        frame: None,
    },
    Expect {
        file: "orf-olympus-e-p2.orf",
        model: "E-P2",
        lens: Some("OLYMPUS M.14-42mm F3.5-5.6"),
        preview: (3200, 2400),
        orientation: Orientation::Normal,
        frame: Some(([0, 168, 4031, 2855], [4032, 3024])),
    },
    Expect {
        file: "orf-olympus-e-m10-mark-iv.orf",
        model: "E-M10MarkIV",
        lens: Some("OLYMPUS M.14-42mm F3.5-5.6 EZ"),
        preview: (3200, 2400),
        orientation: Orientation::Normal,
        frame: Some(([0, 484, 5183, 3403], [5184, 3888])),
    },
    Expect {
        file: "orf-olympus-tg-6.orf",
        model: "TG-6",
        lens: None,
        preview: (3200, 2400),
        orientation: Orientation::Normal,
        frame: Some(([0, 164, 3999, 2835], [4000, 3000])),
    },
    Expect { file: "orf-olympus-e-3.orf", model: "E-3", lens: None, preview: (1600, 1200), orientation: Orientation::Normal, frame: None },
];

#[test]
fn compressed_olympus_files_give_their_jpeg_lens_orientation_and_aspect_frame() {
    let mut seen = 0;
    for e in FILES {
        let path = corpus().join("raw").join(e.file);
        let Ok(bytes) = std::fs::read(&path) else {
            lightcraft_testkit::skip!(Corpus, "{} absent", path.display());
            continue;
        };
        let name = e.file;
        assert!(matches!(decode(&bytes), Err(RawError::Unsupported(_))), "{name}: compressed, shown from its JPEG");
        let jpeg = embedded_preview(&bytes).unwrap_or_else(|| panic!("{name}: no camera JPEG"));
        assert_eq!(jpeg_size(&jpeg), e.preview, "{name}: the largest JPEG of the file");
        let m = lightcraft_meta::extract(&bytes);
        assert_eq!(m.model.as_deref(), Some(e.model), "{name}");
        assert_eq!(m.lens_model.as_deref(), e.lens, "{name}: lens");
        assert_eq!(m.orientation, Some(e.orientation), "{name}");
        let expected = e.frame.map(|([l, t, r, b], [w, h])| {
            let (w, h) = (f64::from(w), f64::from(h));
            [f64::from(l) / w, f64::from(t) / h, f64::from(r + 1) / w, f64::from(b + 1) / h]
        });
        assert_eq!(olympus_aspect_frame(&bytes), expected, "{name}: aspect frame");
        seen += 1;
    }
    eprintln!("compressed Olympus files checked: {seen}");
}

/// A raw that decodes states its aspect frame the same way: the XZ-10, shot at 3:2, develops its whole 3968 × 2976 valid
/// area and says its own JPEG is rows 165 to 2810 of it; the XZ-2 and the E-1 (shot at 4:3, and the older note) say none.
#[test]
fn a_decoded_olympus_raw_states_its_aspect_frame_too() {
    let read = |name: &str| {
        let path = corpus().join("raw").join(name);
        std::fs::read(&path).map_err(|_| lightcraft_testkit::skip!(Corpus, "{} absent", path.display())).ok()
    };
    if let Some(xz10) = read("orf-olympus-xz-10.orf") {
        let raw = decode(&xz10).unwrap();
        assert_eq!((raw.active_area.width, raw.active_area.height), (3968, 2976));
        assert_eq!(olympus_aspect_frame(&xz10), Some([0.0, 165.0 / 2976.0, 1.0, 2811.0 / 2976.0]));
    }
    for name in ["orf-olympus-xz2.orf", "orf-olympus-e1.orf"] {
        if let Some(bytes) = read(name) {
            assert!(decode(&bytes).is_ok(), "{name}");
            assert_eq!(olympus_aspect_frame(&bytes), None, "{name}");
        }
    }
}
