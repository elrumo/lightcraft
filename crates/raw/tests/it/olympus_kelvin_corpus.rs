//! White balance in Kelvin from the preset table of an Olympus / OM System maker note, on CC0 files from raw.pixls.us (URLs and
//! SHA-256 in `RAW_SAMPLES`, xtask/src/main.rs; downloaded into the gitignored corpus/raw by `cargo xtask corpus --download`).
//! Missing files are reported as skipped.
//!
//! The table (ImageProcessing `WB_RBLevels<K>`, twelve entries, see `vendor/orf.rs`) is in the maker note of every body from the
//! E-1 (2003) to the OM-3 (2025) that was checked, 58 of the archive's 70 Olympus files (the others are the compacts with a
//! two-field layout, the E-10, the E-20 and the C-series, which state none). The camera's own stated temperature is the check: when a body
//! was set to a Kelvin value or a preset (`WhiteBalanceTemperature`, read with exiftool run as a black-box tool), its as-shot gains
//! must come out of the curve at that temperature. The compressed files decode no sensor data, but the table is in their maker note.

use lightcraft_raw::{RawFormat, decode, olympus_as_shot_kelvin, probe, probe_info};
use std::path::{Path, PathBuf};

fn corpus() -> PathBuf {
    std::env::var_os("LIGHTCRAFT_CORPUS").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"))
}

fn read(name: &str) -> Option<Vec<u8>> {
    let path = corpus().join("raw").join(name);
    let bytes = std::fs::read(&path).ok();
    if bytes.is_none() {
        lightcraft_testkit::skip!(Corpus, "{} absent", path.display());
    }
    bytes
}

fn mired(kelvin: f64) -> f64 {
    1e6 / kelvin
}

/// Files shot with a stated temperature (a preset or the Kelvin setting): the curve reads it back to within 2 mired (a
/// 5000 K shot reads 4971 K, a 6400 K shot 6361 K), with no tint to speak of.
#[test]
fn a_stated_temperature_is_read_back_from_the_presets() {
    // (file, camera, stated K)
    let cases = [
        ("orf-olympus-e-p3.orf", "E-P3", 5300.0),
        ("orf-olympus-e-pl6.orf", "E-PL6", 5600.0),
        ("orf-olympus-e-pl7.orf", "E-PL7", 5000.0),
        ("orf-olympus-e-m10-mark-iiis.orf", "E-M10MarkIIIS", 6400.0),
        ("orf-olympus-e-pl10.orf", "E-PL10", 7500.0),
        ("orf-om-system-om-5.orf", "OM-5", 6000.0),
    ];
    let mut seen = 0;
    for (name, model, stated) in cases {
        let Some(bytes) = read(name) else { continue };
        assert_eq!(probe(&bytes), Some(RawFormat::Orf), "{name}");
        assert_eq!(lightcraft_meta::extract(&bytes).model.as_deref(), Some(model), "{name}");
        let (t, tint) = olympus_as_shot_kelvin(&bytes).unwrap_or_else(|| panic!("{name}: no temperature"));
        assert!((mired(t) - mired(stated)).abs() < 2.0, "{name}: stated {stated} K, read {t:.0} K");
        assert!(tint.abs() < 2.0, "{name}: tint {tint}");
        seen += 1;
    }
    eprintln!("stated temperatures checked: {seen}");
}

/// The bodies that decode today: the table reaches the decoded raw as its Kelvin scale, header probe and full decode agree, the
/// reading is the one the maker note alone gives, and the camera is one the Kelvin scale covers. The E-1 file was shot on its 5300 K
/// preset: its as-shot gains are that preset's entry.
#[test]
fn the_bodies_that_decode_edit_white_balance_in_kelvin() {
    // (file, Exif model, a window for the as-shot temperature in K, the largest tint). The E-M5 Mark II high-resolution file was
    // shot on a custom white balance: warm and 23 tint units off the daylight curve.
    let cases = [
        ("orf-olympus-e1.orf", "E-1", 5290.0..5310.0, 1.0),
        ("orf-olympus-e-300.orf", "E-300", 6100.0..6200.0, 6.0),
        ("orf-olympus-e-330.orf", "E-330", 5300.0..5450.0, 6.0),
        ("orf-olympus-e400.orf", "E-400", 5050.0..5170.0, 6.0),
        ("orf-olympus-e-500.orf", "E-500", 6250.0..6380.0, 6.0),
        ("orf-olympus-xz2.orf", "XZ-2", 5750.0..5850.0, 6.0),
        ("orf-olympus-pen-f.orf", "PEN-F", 4300.0..4430.0, 6.0),
        ("orf-olympus-e-m5-mark-ii.orf", "E-M5MarkII", 4200.0..4310.0, 26.0),
    ];
    let mut seen = 0;
    for (name, model, window, max_tint) in cases {
        let Some(bytes) = read(name) else { continue };
        let raw = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(probe_info(&bytes).unwrap(), raw.info(), "{name}: the header probe differs from the decode");
        let presets = raw.color.kelvin_presets.as_ref().unwrap_or_else(|| panic!("{name}: no presets"));
        let (t, tint) = presets.temp_tint(raw.wb_multipliers.unwrap_or_else(|| panic!("{name}: no as-shot gains"))).unwrap();
        assert!(window.contains(&t), "{name}: {t:.0} K");
        assert!(tint.abs() <= max_tint, "{name}: tint {tint}");
        assert_eq!(olympus_as_shot_kelvin(&bytes), Some((t, tint)), "{name}: the maker note alone reads the same");
        let m = lightcraft_meta::extract(&bytes);
        assert_eq!(m.model.as_deref(), Some(model), "{name}");
        assert!(lightcraft_raw::kelvin_presets::covers(m.make.as_deref(), model), "{name}: not covered");
        seen += 1;
    }
    eprintln!("decoded bodies checked: {seen}");
}

/// Bodies whose maker note has no such table (the E-10, E-20, C-series and the two-field compacts) get none, and nothing
/// reads a temperature from them: they keep editing relative to the as-shot look.
#[test]
fn bodies_without_a_table_have_no_kelvin_scale() {
    let mut seen = 0;
    for (name, model) in [
        ("orf-olympus-e-10.orf", "E-10"),
        ("orf-olympus-e-20.orf", "E-20,E-20N,E-20P"),
        ("orf-olympus-c5050z.orf", "C5050Z"),
        ("orf-olympus-sp350.orf", "SP350"),
    ] {
        let Some(bytes) = read(name) else { continue };
        let raw = decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(raw.color.kelvin_presets.is_none(), "{name}");
        assert_eq!(olympus_as_shot_kelvin(&bytes), None, "{name}");
        let m = lightcraft_meta::extract(&bytes);
        assert!(!lightcraft_raw::kelvin_presets::covers(m.make.as_deref(), m.model.as_deref().unwrap_or(model)), "{name}");
        seen += 1;
    }
    eprintln!("bodies without a table checked: {seen}");
}
