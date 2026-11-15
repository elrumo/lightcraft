use crate::convert::{from_craft, to_craft};
use crate::{Metadata, Orientation};
use craft_tiff::{Ifd, ParseOptions, Tiff, TiffError, tags as t};

pub fn strip_exif_header(b: &[u8]) -> &[u8] {
    craft_metadata::strip_exif_header(b)
}

pub fn read_exif(bytes: &[u8]) -> Metadata {
    with_olympus_lens(from_craft(craft_metadata::read_exif(bytes)), bytes, &ParseOptions::default())
}

pub fn try_read_exif(bytes: &[u8]) -> Result<Metadata, TiffError> {
    craft_metadata::try_read_exif(bytes).map(|m| with_olympus_lens(from_craft(m), bytes, &ParseOptions::default()))
}

pub fn try_read_exif_with(bytes: &[u8], opts: &ParseOptions) -> Result<Metadata, TiffError> {
    craft_metadata::try_read_exif_with(bytes, opts).map(|m| with_olympus_lens(from_craft(m), bytes, opts))
}

/// The Olympus bodies before the E-M5 (E-30, E-5, E-420, E-450, E-520, E-600, E-620, E-P1 to E-PL3, E-PM1) write no
/// Exif LensModel, only the name in their maker note: read it there when the Exif has none.
fn with_olympus_lens(mut m: Metadata, bytes: &[u8], opts: &ParseOptions) -> Metadata {
    let olympus = m.make.as_deref().is_some_and(|make| {
        let make = make.trim_start().to_ascii_uppercase();
        make.starts_with("OLYMPUS") || make.starts_with("OM ")
    });
    if m.lens_model.is_none() && olympus {
        let data = strip_exif_header(bytes);
        if let Ok(tiff) = Tiff::parse_with(data, opts) {
            m.lens_model = olympus_maker_note_lens(data, &tiff);
        }
    }
    m
}

fn text(ifd: Option<&Ifd>, tag: u16) -> Option<String> {
    let s = ifd?.string(tag)?;
    let s = s.trim().to_string();
    (!s.is_empty() && !s.bytes().all(|b| b == b' ' || b == 0)).then_some(s)
}

/// Olympus maker note tag `0x2010` (the Equipment directory in the ExifTool Olympus tag names, an IFD pointer counted from
/// the note's start) and its tag `0x0203` (LensModel, ASCII).
const OLYMPUS_EQUIPMENT: u16 = 0x2010;
const OLYMPUS_LENS_MODEL: u16 = 0x0203;

/// The lens name in the Equipment directory of a new-style Olympus or OM System maker note (`OLYMPUS\0` or `OM SYSTEM\0`) of
/// this Exif stream (`data` starts at its TIFF header). The older `OLYMP\0` notes name the lens by a type code only.
pub(crate) fn olympus_maker_note_lens(data: &[u8], tiff: &Tiff) -> Option<String> {
    let exif = tiff.exif()?;
    let note = exif.bytes(t::MAKER_NOTE)?;
    if !(note.starts_with(b"OLYMPUS\0") || note.starts_with(b"OM SYSTEM\0")) {
        return None;
    }
    let e = exif.get(t::MAKER_NOTE)?;
    let make = tiff.ifds.first().and_then(|i| i.string(t::MAKE)).unwrap_or_default();
    let mn = craft_tiff::makernote::parse_makernote(data, e.offset, e.count() as u64, tiff.order, &make)?;
    let at = mn.base.checked_add(mn.ifd.u64(OLYMPUS_EQUIPMENT)?)?;
    let opts = ParseOptions { max_ifds: 4, max_depth: 1, follow_children: false, ..Default::default() };
    let (equipment, _) = craft_tiff::parse_ifd_at(data, at, mn.order, mn.base, false, &opts).ok()?;
    text(Some(&equipment), OLYMPUS_LENS_MODEL)
}

pub fn from_tiff(tiff: &Tiff) -> Metadata {
    from_craft(craft_metadata::from_tiff(tiff))
}

pub fn write_exif(meta: &Metadata) -> Vec<u8> {
    craft_metadata::write_exif(&to_craft(meta))
}

pub(crate) fn ifd0_orientation(bytes: &[u8]) -> Option<Orientation> {
    let tiff = Tiff::parse(strip_exif_header(bytes)).ok()?;
    tiff.ifds.first()?.u16(t::ORIENTATION).map(Orientation::from_exif)
}

#[cfg(test)]
mod write_tests {
    use super::*;
    use crate::{DateTime, Gps};

    #[test]
    fn capture_time_offset_and_subsec_round_trip() {
        for iso in ["2026-09-20T05:40:00.250+02:00", "2026-09-20T05:40:00.005-03:30", "2026-09-20T05:40:00"] {
            let m = Metadata { capture_time: DateTime::parse_iso(iso), ..Default::default() };
            assert_eq!(read_exif(&write_exif(&m)).capture_time, m.capture_time, "{iso}");
        }
    }

    /// A deserialized offset can be any i16, `i16::MIN` included: writing must not overflow.
    #[test]
    fn extreme_capture_time_offsets_do_not_panic() {
        for o in [i16::MIN, i16::MAX, -1081, 1081] {
            let mut dt = DateTime::parse_iso("2026-09-20T05:40:00").unwrap();
            dt.offset_minutes = Some(o);
            let m = Metadata { capture_time: Some(dt), ..Default::default() };
            assert_eq!(read_exif(&write_exif(&m)).capture_time.map(|d| d.offset_minutes), Some(None), "{o}");
        }
    }

    #[test]
    fn exif_round_trips() {
        let m = Metadata {
            make: Some("Synthetic".into()),
            model: Some("X2".into()),
            copyright: Some("© 2026 A. Person".into()),
            artist: Some("A. Person".into()),
            capture_time: DateTime::parse_iso("2026-09-20T05:40:00"),
            exposure_time: Some(1.0 / 250.0),
            f_number: Some(2.8),
            iso: Some(400),
            focal_length: Some(35.0),
            lens_model: Some("35mm F2".into()),
            gps: Some(Gps { latitude: 43.7904, longitude: -110.6818, altitude: Some(2000.0) }),
            ..Default::default()
        };
        let back = read_exif(&write_exif(&m));
        assert_eq!(back.make, m.make);
        assert_eq!(back.model, m.model);
        assert_eq!(back.copyright, m.copyright);
        assert_eq!(back.artist, m.artist);
        assert_eq!(back.capture_time.map(|d| d.to_exif()), m.capture_time.map(|d| d.to_exif()));
        assert_eq!(back.iso, Some(400));
        assert!((back.f_number.unwrap() - 2.8).abs() < 1e-6);
        assert!((back.exposure_time.unwrap() - 0.004).abs() < 1e-9);
        assert_eq!(back.lens_model, m.lens_model);
        let g = back.gps.unwrap();
        assert!((g.latitude - 43.7904).abs() < 1e-5 && (g.longitude + 110.6818).abs() < 1e-5, "{g:?}");
    }

    #[test]
    fn iso_above_65534_round_trips() {
        for iso in [65_534, 65_535, 102_400, 409_600] {
            let back = read_exif(&write_exif(&Metadata { iso: Some(iso), ..Default::default() }));
            assert_eq!(back.iso, Some(iso));
        }
    }

    #[test]
    fn fast_shutter_speeds_round_trip_exactly() {
        for den in [8000.0, 12_800.0, 16_000.0, 32_000.0] {
            let back = read_exif(&write_exif(&Metadata { exposure_time: Some(1.0 / den), ..Default::default() }));
            let got = back.exposure_time.unwrap();
            assert!((1.0 / got - den).abs() < 0.5, "1/{den} s came back as 1/{}", 1.0 / got);
        }
    }

    #[test]
    fn written_shutter_speeds_show_as_the_same_reciprocal() {
        // #1619 / #2542: the exact time is what export writes, and it reads back as the text shown
        for (t, want) in [(1.0 / 3.0, "1/3"), (1.0 / 250.0, "1/250"), (1.0 / 12_800.0, "1/12800"), (0.3, "0.3"), (1.0 / 2.8, "0.3571")] {
            let back = read_exif(&write_exif(&Metadata { exposure_time: Some(t), ..Default::default() }));
            let got = back.exposure_time.unwrap();
            assert_eq!(crate::format_exposure_time(got).as_deref(), Some(want), "{t} came back as {got}");
        }
    }
}

#[cfg(test)]
mod olympus_tests {
    use super::*;
    use craft_tiff::{ByteOrder, IfdBuilder, TiffWriter, Value};

    /// An Exif block from an Olympus body that writes no Exif LensModel: a new-style maker note (`magic` is its header, the
    /// IFD right after it, offsets counted from the note's start) with the Equipment directory (0x2010) holding the lens
    /// name in tag 0x0203, or, for `lens` `None`, another entry in its place.
    fn olympus_exif(magic: &[u8], lens: Option<&str>, lens_model: Option<&str>) -> Vec<u8> {
        let entry = |tag: u16, typ: u16, count: u32, value: u32| {
            let mut e = tag.to_le_bytes().to_vec();
            e.extend(typ.to_le_bytes());
            e.extend(count.to_le_bytes());
            e.extend(value.to_le_bytes());
            e
        };
        let equipment_at = magic.len() + 2 + 12 + 4;
        let name_at = equipment_at + 2 + 12 + 4;
        let mut note = magic.to_vec();
        note.extend(1u16.to_le_bytes());
        note.extend(entry(OLYMPUS_EQUIPMENT, 13, 1, equipment_at as u32));
        note.extend(0u32.to_le_bytes());
        assert_eq!(note.len(), equipment_at);
        note.extend(1u16.to_le_bytes());
        let name = format!("{}\0", lens.unwrap_or(""));
        note.extend(match lens {
            Some(_) => entry(OLYMPUS_LENS_MODEL, 2, name.len() as u32, name_at as u32),
            None => entry(0x0204, 4, 1, 0),
        });
        note.extend(0u32.to_le_bytes());
        note.extend(name.as_bytes());
        let mut exif = IfdBuilder::new();
        exif.set(t::MAKER_NOTE, Value::Undefined(note));
        exif.set(t::FOCAL_LENGTH, Value::Rational(vec![(12, 1)]));
        if let Some(l) = lens_model {
            exif.set(t::LENS_MODEL, Value::Ascii(l.into()));
        }
        let mut ifd0 = IfdBuilder::new();
        ifd0.set(t::MAKE, Value::Ascii("OLYMPUS IMAGING CORP.".into()));
        ifd0.set_child(t::EXIF_IFD, exif);
        TiffWriter::new(ByteOrder::Little, false).write(&[ifd0]).unwrap()
    }

    const OLYMPUS_NOTE: &[u8] = b"OLYMPUS\0II\x03\0";
    const OM_SYSTEM_NOTE: &[u8] = b"OM SYSTEM\0\0\0II\x04\0";

    #[test]
    fn olympus_maker_note_names_the_lens_when_exif_does_not() {
        // E-P2, E-30, E-5 ...: no Exif LensModel, the name only in Equipment; OM System bodies use the same directory
        for magic in [OLYMPUS_NOTE, OM_SYSTEM_NOTE] {
            let m = read_exif(&olympus_exif(magic, Some("OLYMPUS M.14-42mm F3.5-5.6"), None));
            assert_eq!(m.lens_model.as_deref(), Some("OLYMPUS M.14-42mm F3.5-5.6"));
            assert_eq!(m.focal_length, Some(12.0));
        }
        // Exif's own LensModel wins; a note without the tag, or a blank name (an adapted lens), leaves the lens empty
        let m = read_exif(&olympus_exif(OLYMPUS_NOTE, Some("OLYMPUS M.14-42mm F3.5-5.6"), Some("Other 25mm")));
        assert_eq!(m.lens_model.as_deref(), Some("Other 25mm"));
        assert_eq!(read_exif(&olympus_exif(OLYMPUS_NOTE, None, None)).lens_model, None);
        assert_eq!(read_exif(&olympus_exif(OLYMPUS_NOTE, Some("        "), None)).lens_model, None);
        // the same bytes under another maker's note header read nothing
        assert_eq!(read_exif(&olympus_exif(b"Panasonic\0\0\0II\x03\0", Some("not Olympus"), None)).lens_model, None);
    }

    /// A hostile note (a pointer past the file or to itself, a cut-short file) gives no lens, never a panic.
    #[test]
    fn olympus_equipment_pointer_and_truncation_never_panic() {
        let good = olympus_exif(OLYMPUS_NOTE, Some("OLYMPUS M.12-40mm F2.8"), None);
        assert_eq!(read_exif(&good).lens_model.as_deref(), Some("OLYMPUS M.12-40mm F2.8"));
        let at = good.windows(8).position(|w| w == b"OLYMPUS\0").unwrap();
        // the pointer (the IFD's first entry's value, 12 + 2 + 8 bytes into the note): every extreme
        for ptr in [0u32, 1, 12, 13, u32::MAX, u32::MAX - 3, 0x7fff_ffff, good.len() as u32, good.len() as u32 + 100] {
            let mut bad = good.clone();
            bad[at + 12 + 2 + 8..at + 12 + 2 + 12].copy_from_slice(&ptr.to_le_bytes());
            let _ = read_exif(&bad);
        }
        for len in 0..good.len() {
            let _ = read_exif(&good[..len]);
        }
    }
}
