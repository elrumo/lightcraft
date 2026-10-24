//! Malformed vendor raw files (NEF, ARW, PEF, ORF, RW2 of every raw format, CR2, CR3, RAF) must decode to an error, never panic.

use lightcraft_tiff::tags as t;
use lightcraft_tiff::{ByteOrder, IfdBuilder, ImageData, TiffWriter, Value};
use proptest::prelude::*;

const W: u32 = 32;
const H: u32 = 8;

/// A CFA TIFF the way most vendors lay it out: IFD0 (make) + a SubIFD holding the raw strip.
fn cfa_tiff(make: &str, compression: u16, bits: u16, order: ByteOrder) -> Vec<u8> {
    let px = (W * H) as usize;
    // Sony's ARW2 (32767) packs a sample per byte
    let len = if compression == 32767 { px } else { px * 2 };
    let strip: Vec<u8> = (0..len).map(|i| (i * 37 % 251) as u8).collect();
    let mut raw = IfdBuilder::new();
    raw.set(t::NEW_SUBFILE_TYPE, Value::Long(vec![0]));
    raw.set(t::IMAGE_WIDTH, Value::Long(vec![W]));
    raw.set(t::IMAGE_LENGTH, Value::Long(vec![H]));
    raw.set(t::BITS_PER_SAMPLE, Value::Short(vec![bits]));
    raw.set(t::COMPRESSION, Value::Short(vec![compression]));
    raw.set(t::PHOTOMETRIC, Value::Short(vec![t::photometric::CFA]));
    raw.set(t::CFA_REPEAT_PATTERN_DIM, Value::Short(vec![2, 2]));
    raw.set(t::CFA_PATTERN_EP, Value::Byte(vec![0, 1, 1, 2]));
    raw.set_image(ImageData::Strips { rows_per_strip: H, strips: vec![strip] });
    let mut ifd0 = IfdBuilder::new();
    ifd0.set(t::MAKE, Value::Ascii(make.into()));
    ifd0.set(t::MODEL, Value::Ascii("TEST".into()));
    ifd0.add_sub_ifd(raw);
    TiffWriter::new(order, false).write(&[ifd0]).unwrap()
}

/// An Olympus ORF (TIFF with an `IIRO` magic) or Panasonic RW2 (`IIU\0`), raw image in IFD0.
fn magic_tiff(magic: &[u8; 4], make: &str) -> Vec<u8> {
    let px = (W * H) as usize;
    let mut ifd = IfdBuilder::new();
    ifd.set(t::IMAGE_WIDTH, Value::Long(vec![W]));
    ifd.set(t::IMAGE_LENGTH, Value::Long(vec![H]));
    ifd.set(t::BITS_PER_SAMPLE, Value::Short(vec![16]));
    ifd.set(t::COMPRESSION, Value::Short(vec![1]));
    ifd.set(t::PHOTOMETRIC, Value::Short(vec![1]));
    ifd.set(t::MAKE, Value::Ascii(make.into()));
    ifd.set_image(ImageData::Strips { rows_per_strip: H, strips: vec![(0..px * 2).map(|i| (i * 13) as u8).collect()] });
    let mut b = TiffWriter::new(ByteOrder::Little, false).write(&[ifd]).unwrap();
    b[..4].copy_from_slice(magic);
    b
}

/// An Olympus ORF with a new-style maker note (`OLYMPUS\0II\x03\0`, offsets counted from the note's start): the Equipment
/// directory with a lens name, CameraSettings pointing at a JPEG-looking preview and ImageProcessing with a valid area,
/// an aspect frame, a black level and white balance. Its strip is the compressed kind (short) or uncompressed words.
fn olympus_noted(uncompressed: bool) -> Vec<u8> {
    let entry = |tag: u16, typ: u16, count: u32, value: u32| {
        let mut e = tag.to_le_bytes().to_vec();
        e.extend(typ.to_le_bytes());
        e.extend(count.to_le_bytes());
        e.extend(value.to_le_bytes());
        e
    };
    let lens = b"OLYMPUS M.12-40mm F2.8\0";
    let jpeg = [&[0xffu8, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 11, 8, 0, 4, 0, 4, 1, 1, 0x11, 0][..], &[0xff, 0xd9]].concat();
    // layout: header 12 | main IFD (3 entries) | Equipment (1) | CameraSettings (2) | ImageProcessing (7) | frame, wb | lens | jpeg
    let main_at = 12;
    let equipment_at = main_at + 2 + 3 * 12 + 4;
    let settings_at = equipment_at + 2 + 12 + 4;
    let processing_at = settings_at + 2 + 2 * 12 + 4;
    let values_at = processing_at + 2 + 7 * 12 + 4;
    let (frame_at, wb_at, black_at, lens_at) = (values_at, values_at + 8, values_at + 16, values_at + 24);
    let jpeg_at = lens_at + lens.len();
    let mut n = b"OLYMPUS\0II\x03\0".to_vec();
    n.extend(3u16.to_le_bytes());
    n.extend(entry(0x2010, 13, 1, equipment_at as u32));
    n.extend(entry(0x2020, 13, 1, settings_at as u32));
    n.extend(entry(0x2040, 13, 1, processing_at as u32));
    n.extend(0u32.to_le_bytes());
    n.extend(1u16.to_le_bytes());
    n.extend(entry(0x0203, 2, lens.len() as u32, lens_at as u32));
    n.extend(0u32.to_le_bytes());
    n.extend(2u16.to_le_bytes());
    n.extend(entry(0x0101, 4, 1, jpeg_at as u32));
    n.extend(entry(0x0102, 4, 1, jpeg.len() as u32));
    n.extend(0u32.to_le_bytes());
    n.extend(7u16.to_le_bytes());
    n.extend(entry(0x0100, 3, 4, wb_at as u32));
    n.extend(entry(0x0600, 3, 4, black_at as u32));
    n.extend(entry(0x0612, 4, 1, 0));
    n.extend(entry(0x0613, 4, 1, 0));
    n.extend(entry(0x0614, 4, 1, W));
    n.extend(entry(0x0615, 4, 1, H));
    n.extend(entry(0x1113, 3, 4, frame_at as u32));
    n.extend(0u32.to_le_bytes());
    n.extend([0u16, 1, W as u16 - 1, H as u16 - 2].iter().flat_map(|v| v.to_le_bytes()));
    n.extend([500u16, 440, 256, 256].iter().flat_map(|v| v.to_le_bytes()));
    n.extend([64u16, 65, 65, 64].iter().flat_map(|v| v.to_le_bytes()));
    n.extend(lens);
    n.extend(&jpeg);
    let px = (W * H) as usize;
    let mut ifd = IfdBuilder::new();
    ifd.set(t::IMAGE_WIDTH, Value::Long(vec![W]));
    ifd.set(t::IMAGE_LENGTH, Value::Long(vec![H]));
    ifd.set(t::BITS_PER_SAMPLE, Value::Short(vec![16]));
    ifd.set(t::COMPRESSION, Value::Short(vec![1]));
    ifd.set(t::PHOTOMETRIC, Value::Short(vec![1]));
    ifd.set(t::MAKE, Value::Ascii("OLYMPUS IMAGING CORP.".into()));
    ifd.set(t::MODEL, Value::Ascii("E-M1MarkII".into()));
    let strip = if uncompressed { (0..px * 2).map(|i| (i * 13) as u8).collect() } else { vec![0, 0, 0, 0, 1, 0, 0, 9, 9, 9] };
    ifd.set_image(ImageData::Strips { rows_per_strip: H, strips: vec![strip] });
    let mut exif = IfdBuilder::new();
    exif.set(0xa302, Value::Undefined(vec![0, 2, 0, 2, 0, 1, 1, 2]));
    exif.set(t::MAKER_NOTE, Value::Undefined(n));
    ifd.set_child(t::EXIF_IFD, exif);
    let mut b = TiffWriter::new(ByteOrder::Little, false).write(&[ifd]).unwrap();
    b[..4].copy_from_slice(b"IIRO");
    b
}

/// A Panasonic RW2: sensor size, bit depth and raw format in IFD0, 12-bit packed blocks.
fn rw2() -> Vec<u8> {
    let mut ifd = IfdBuilder::new();
    ifd.set(0x0002, Value::Short(vec![W as u16]));
    ifd.set(0x0003, Value::Short(vec![H as u16]));
    ifd.set(0x0004, Value::Short(vec![1]));
    ifd.set(0x0005, Value::Short(vec![2]));
    ifd.set(0x0006, Value::Short(vec![H as u16 - 1]));
    ifd.set(0x0007, Value::Short(vec![W as u16 - 2]));
    ifd.set(0x000a, Value::Short(vec![12]));
    ifd.set(0x002d, Value::Short(vec![5]));
    ifd.set(0x001c, Value::Short(vec![64]));
    ifd.set(0x001d, Value::Short(vec![64]));
    ifd.set(0x001e, Value::Short(vec![64]));
    ifd.set(t::MAKE, Value::Ascii("Panasonic".into()));
    let need = (W * H).div_ceil(10) as usize * 16;
    ifd.set_image(ImageData::Strips { rows_per_strip: H, strips: vec![(0..need).map(|i| (i * 29) as u8).collect()] });
    let mut b = TiffWriter::new(ByteOrder::Little, false).write(&[ifd]).unwrap();
    b[..4].copy_from_slice(b"IIU\0");
    b
}

/// A Panasonic RW2 of raw format `format` (or, with `format` 0, the oldest 16-bit-word kind): sensor size, borders,
/// bit depth, black levels, an in-camera crop and `n` bytes of payload at the end, at `RawDataOffset`. Format 8 also
/// gets two strips covering the payload and a code table.
fn rw2_of(format: u16, bits: u16, n: usize) -> Vec<u8> {
    let u16s = |v: &[u16]| Value::Undefined(v.iter().flat_map(|x| x.to_le_bytes()).collect());
    let build = |off: u32| {
        let mut ifd = IfdBuilder::new();
        for (tag, v) in [(2, W as u16), (3, H as u16), (4, 2), (5, 4), (6, H as u16), (7, W as u16), (9, 2), (0x0a, bits), (0x1c, 128), (0x1d, 128)] {
            ifd.set(tag, Value::Short(vec![v]));
        }
        for (tag, v) in [(0x2f, 2u16), (0x30, 6), (0x31, H as u16 - 2), (0x32, W as u16 - 2)] {
            ifd.set(tag, Value::Short(vec![v]));
        }
        if format == 0 {
            ifd.set(0x000b, Value::Short(vec![34828]));
            ifd.set(t::STRIP_OFFSETS, Value::Long(vec![off]));
        } else {
            ifd.set(0x002d, Value::Short(vec![format]));
            ifd.set(0x0118, Value::Long(vec![off]));
        }
        if format == 8 {
            let table = [
                (6, 62),
                (7, 126),
                (6, 61),
                (5, 28),
                (4, 12),
                (3, 4),
                (3, 2),
                (2, 0),
                (3, 3),
                (3, 5),
                (4, 13),
                (5, 29),
                (6, 60),
                (8, 254),
                (8, 255),
                (12, 4094),
                (12, 4095),
            ];
            ifd.set(0x0040, u16s(&std::iter::once(17).chain(table.iter().flat_map(|&(l, c)| [l, c])).collect::<Vec<u16>>()));
            let half = (n / 2) as u32;
            let (a, b) = (off, off + half);
            ifd.set(0x0044, u16s(&[2, a as u16, (a >> 16) as u16, b as u16, (b >> 16) as u16]));
            ifd.set(0x0045, u16s(&[2, 0, 0, W as u16 / 2, 0]));
            ifd.set(0x0046, u16s(&[2, (half * 8) as u16, ((half * 8) >> 16) as u16, (half * 8) as u16, ((half * 8) >> 16) as u16]));
            ifd.set(0x0047, u16s(&[2, W as u16 / 2, W as u16 / 2]));
            ifd.set(0x0048, u16s(&[2, H as u16, H as u16]));
        }
        ifd.set(t::MAKE, Value::Ascii("Panasonic".into()));
        let mut b = TiffWriter::new(ByteOrder::Little, false).write(&[ifd]).unwrap();
        b[..4].copy_from_slice(b"IIU\0");
        b
    };
    let off = build(0).len();
    let mut b = build(off as u32);
    b.extend((0..n).map(|i| (i * 89 % 253) as u8));
    b
}

/// A Pentax PEF: the raw image in IFD0, wide enough for the dark-border trim.
fn pef(compression: u16) -> Vec<u8> {
    let (w, h) = (264u32, 4u32);
    let mut ifd = IfdBuilder::new();
    ifd.set(t::MAKE, Value::Ascii("PENTAX".into()));
    ifd.set(t::IMAGE_WIDTH, Value::Long(vec![w]));
    ifd.set(t::IMAGE_LENGTH, Value::Long(vec![h]));
    ifd.set(t::BITS_PER_SAMPLE, Value::Short(vec![16]));
    ifd.set(t::COMPRESSION, Value::Short(vec![compression]));
    ifd.set(t::PHOTOMETRIC, Value::Short(vec![t::photometric::CFA]));
    let strip: Vec<u8> = (0..(w * h * 2) as usize).map(|i| if i % 528 < 8 { 0 } else { (i * 7 % 200) as u8 }).collect();
    ifd.set_image(ImageData::Strips { rows_per_strip: h, strips: vec![strip] });
    TiffWriter::new(ByteOrder::Big, false).write(&[ifd]).unwrap()
}

/// A Canon CR2: three placeholder IFDs and IFD3 with a 2-component lossless JPEG frame.
fn cr2() -> Vec<u8> {
    let img: Vec<u16> = (0..(W * H) as usize).map(|i| 1024 + (i * 7919 % 12000) as u16).collect();
    let enc = lightcraft_raw::ljpeg::encode(&img, W as usize / 2, H as usize, 2, 14, 1, 0);
    let mut ifd0 = IfdBuilder::new();
    ifd0.set(t::MAKE, Value::Ascii("Canon".into()));
    let mut ifd3 = IfdBuilder::new();
    ifd3.set(t::COMPRESSION, Value::Short(vec![6]));
    ifd3.set(0xc640, Value::Short(vec![1, 16, 16]));
    ifd3.set_image(ImageData::Strips { rows_per_strip: H, strips: vec![enc] });
    let blank = || IfdBuilder::new().with(1, Value::Short(vec![0]));
    TiffWriter::new(ByteOrder::Little, false).write(&[ifd0, blank(), blank(), ifd3]).unwrap()
}

/// A Fujifilm RAF: header, records, raw block (LE TIFF whose IFD0 points to a 0xf000 raw IFD).
fn raf_file(w: u32, h: u32, bits: u32, strip: Vec<u8>, layout: Option<[u8; 36]>, jpeg: &[u8]) -> Vec<u8> {
    let mut block = b"II*\0\x08\0\0\0".to_vec();
    let sub_off: u32 = 8 + 2 + 12 + 4;
    block.extend_from_slice(&1u16.to_le_bytes());
    block.extend_from_slice(&[0x00, 0xf0, 13, 0, 1, 0, 0, 0]);
    block.extend_from_slice(&sub_off.to_le_bytes());
    block.extend_from_slice(&0u32.to_le_bytes());
    let n = 7u32;
    let wb_off = sub_off + 2 + 12 * n + 4;
    let strip_off = wb_off + 12;
    let entries: [(u16, u32, u32); 7] = [
        (0xf001, 1, w),
        (0xf002, 1, h),
        (0xf003, 1, bits),
        (0xf007, 1, strip_off),
        (0xf008, 1, strip.len() as u32),
        (0xf00a, 1, 64),
        (0xf00e, 3, wb_off),
    ];
    block.extend_from_slice(&(n as u16).to_le_bytes());
    for (tag, count, v) in entries {
        block.extend_from_slice(&tag.to_le_bytes());
        block.extend_from_slice(&4u16.to_le_bytes());
        block.extend_from_slice(&count.to_le_bytes());
        block.extend_from_slice(&v.to_le_bytes());
    }
    block.extend_from_slice(&0u32.to_le_bytes());
    for v in [300u32, 600, 450] {
        block.extend_from_slice(&v.to_le_bytes());
    }
    block.extend_from_slice(&strip);
    let mut recs: Vec<u8> = Vec::new();
    let mut nrec = 0u32;
    let mut add = |tag: u16, d: &[u8]| {
        recs.extend_from_slice(&tag.to_be_bytes());
        recs.extend_from_slice(&(d.len() as u16).to_be_bytes());
        recs.extend_from_slice(d);
        nrec += 1;
    };
    add(0x0100, &[(h >> 8) as u8, h as u8, (w >> 8) as u8, w as u8]);
    add(0x0110, &[0, 2, 0, 4]);
    add(0x0111, &[0, (h - 2) as u8, 0, (w - 4) as u8]);
    if let Some(l) = layout {
        add(0x0131, &l);
    }
    let mut dir = nrec.to_be_bytes().to_vec();
    dir.extend_from_slice(&recs);
    let mut out = b"FUJIFILMCCD-RAW 0201FF000000TEST".to_vec();
    out.resize(108, 0);
    let jpeg_off = 160u32;
    let dir_off = jpeg_off + jpeg.len() as u32;
    let raw_off = dir_off + dir.len() as u32;
    for (i, v) in [jpeg_off, jpeg.len() as u32, dir_off, dir.len() as u32, raw_off, block.len() as u32].iter().enumerate() {
        out[84 + i * 4..88 + i * 4].copy_from_slice(&v.to_be_bytes());
    }
    out.resize(160, 0);
    out.extend_from_slice(jpeg);
    out.extend_from_slice(&dir);
    out.extend_from_slice(&block);
    out
}

fn raf() -> Vec<u8> {
    let strip: Vec<u8> = (0..(W * H * 2) as usize).map(|i| (i * 11) as u8).collect();
    let layout: [u8; 36] = std::array::from_fn(|i| (i % 3) as u8);
    raf_file(W, H, 16, strip, Some(layout), b"\xff\xd8\xff\xd9")
}

/// An ISO-BMFF box.
fn bmff(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// A Canon CR3 with a lossless-CRX raw track: one tile, four planes of noise (so the CRX decoder meets a stream of
/// random codes), a `CMP1` coding header and a `CDI1`/`IAD1` image-area box.
fn cr3(levels: u8) -> Vec<u8> {
    let (w, h) = (W as usize, H as usize);
    let planes: Vec<Vec<u8>> = (0..4u32).map(|p| (0..40u32).map(|i| ((i * 53 + p * 17) as u8) ^ 0x5a).collect()).collect();
    let mut sample = vec![0xff, 0x01, 0, 8];
    sample.extend((planes.iter().map(Vec::len).sum::<usize>() as u32).to_be_bytes());
    sample.extend([0; 4]);
    for (p, plane) in planes.iter().enumerate() {
        sample.extend([0xff, 0x02, 0, 8]);
        sample.extend((plane.len() as u32).to_be_bytes());
        sample.extend([(p as u8) << 4 | 8, 0, 0, 0]);
        sample.extend([0xff, 0x03, 0, 8]);
        sample.extend((plane.len() as u32).to_be_bytes());
        sample.extend([0, 0x20, 0, 1]);
    }
    sample.resize(112, 0);
    sample.extend(planes.concat());
    let mut cmp1 = vec![0xff, 0, 0, 0x30, 1, 0, 0, 0];
    for v in [w, h, w, h] {
        cmp1.extend((v as u32).to_be_bytes());
    }
    cmp1.extend([14, 0x40, levels, 0]);
    cmp1.extend(112u32.to_be_bytes());
    cmp1.extend([1, 1, 0, 0].repeat(4));
    let mut iad = vec![0u8; 4];
    for v in [
        w as u16,
        h as u16,
        1,
        2,
        1,
        0,
        2,
        2,
        w as u16 - 3,
        h as u16 - 3,
        0,
        0,
        3,
        h as u16 - 1,
        4,
        0,
        w as u16 - 1,
        3,
        4,
        4,
        w as u16 - 1,
        h as u16 - 1,
    ] {
        iad.extend(v.to_be_bytes());
    }
    let mut entry = vec![0u8; 82];
    entry[24..26].copy_from_slice(&(w as u16).to_be_bytes());
    entry[26..28].copy_from_slice(&(h as u16).to_be_bytes());
    entry.extend(bmff(b"CMP1", &cmp1));
    entry.extend(bmff(b"CDI1", &[vec![0u8; 4], bmff(b"IAD1", &iad)].concat()));
    let full = |kind: &[u8; 4], body: &[u8]| bmff(kind, &[vec![0u8; 4], body.to_vec()].concat());
    let mut stsd = 1u32.to_be_bytes().to_vec();
    stsd.extend(bmff(b"CRAW", &entry));
    let (mut stsz, mut co64) = (vec![0u8; 4], 1u32.to_be_bytes().to_vec());
    stsz.extend(1u32.to_be_bytes());
    stsz.extend((sample.len() as u32).to_be_bytes());
    co64.extend(2048u64.to_be_bytes());
    let stbl = [full(b"stsd", &stsd), full(b"stsz", &stsz), full(b"co64", &co64)].concat();
    let trak = bmff(b"trak", &bmff(b"mdia", &bmff(b"minf", &bmff(b"stbl", &stbl))));
    let mut f = bmff(b"ftyp", b"crx \0\0\0\x01crx isom");
    f.extend(bmff(b"moov", &trak));
    f.resize(2048, 0);
    f.extend(sample);
    f
}

fn samples() -> Vec<Vec<u8>> {
    vec![
        cfa_tiff("NIKON CORPORATION", 1, 12, ByteOrder::Big),
        cfa_tiff("NIKON CORPORATION", 34713, 14, ByteOrder::Big),
        cfa_tiff("SONY", 32767, 12, ByteOrder::Little),
        cfa_tiff("SONY", 1, 14, ByteOrder::Little),
        cfa_tiff("SONY", 32766, 14, ByteOrder::Little),
        pef(1),
        pef(65535),
        cfa_tiff("SAMSUNG", 1, 12, ByteOrder::Little),
        cfa_tiff("ACME", 1, 16, ByteOrder::Little),
        magic_tiff(b"IIRO", "OLYMPUS IMAGING CORP."),
        olympus_noted(false),
        olympus_noted(true),
        rw2(),
        cr2(),
        raf(),
        cr3(0),
        cr3(3),
    ]
    .into_iter()
    .chain(rw2_formats())
    .collect()
}

/// One RW2 per encoding with a sample decoder of its own (formats 4, 6 at 14 and 12 bits, 8, and 16-bit words).
fn rw2_formats() -> Vec<Vec<u8>> {
    vec![
        rw2_of(4, 12, (W * H).div_ceil(14) as usize * 16),
        rw2_of(6, 14, (W * H).div_ceil(11) as usize * 16),
        rw2_of(6, 12, (W * H).div_ceil(14) as usize * 16),
        rw2_of(8, 14, (W * H) as usize * 2),
        rw2_of(0, 12, (W * H) as usize * 2),
    ]
}

fn exercise(bytes: &[u8]) {
    let _ = lightcraft_raw::probe(bytes);
    let _ = lightcraft_raw::probe_info(bytes);
    let _ = lightcraft_raw::embedded_preview(bytes);
    let _ = lightcraft_raw::olympus_aspect_frame(bytes);
    let _ = lightcraft_meta::extract(bytes);
    if let Ok(img) = lightcraft_raw::decode(bytes)
        && img.width * img.height <= 1 << 16
    {
        let _ = img.develop(lightcraft_raw::Method::Ahd);
    }
}

#[test]
fn vendor_samples_truncated_at_every_length() {
    for s in samples() {
        exercise(&s);
        for n in (0..s.len()).step_by(3) {
            exercise(&s[..n]);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, .. ProptestConfig::default() })]

    #[test]
    fn mutated_vendor_files_never_panic(kind in 0usize..22, flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..16), cut in any::<usize>()) {
        let mut data = samples().swap_remove(kind);
        let n = data.len();
        for (i, v) in flips {
            data[i % n] = v;
        }
        let keep = if cut % 3 == 0 { cut % n } else { n };
        exercise(&data[..keep]);
    }
}

/// The RW2 samples decode as they are, so the mutations above reach every format's sample decoder.
#[test]
fn rw2_samples_decode_unmutated() {
    for s in &rw2_formats() {
        let r = lightcraft_raw::decode(s).unwrap();
        assert_eq!((r.width, r.height), (W as usize, H as usize));
    }
}

/// The Olympus samples are real enough for the mutations above to reach the maker-note readers: the frame, the lens and
/// the preview are found in them as they are, and the uncompressed one develops.
#[test]
fn olympus_samples_read_through_their_maker_note_unmutated() {
    for uncompressed in [false, true] {
        let s = olympus_noted(uncompressed);
        assert_eq!(lightcraft_raw::probe(&s), Some(lightcraft_raw::RawFormat::Orf));
        assert_eq!(lightcraft_raw::olympus_aspect_frame(&s), Some([0.0, 1.0 / H as f64, 1.0, (H - 1) as f64 / H as f64]));
        assert_eq!(lightcraft_meta::extract(&s).lens_model.as_deref(), Some("OLYMPUS M.12-40mm F2.8"));
        assert!(lightcraft_raw::embedded_preview(&s).is_some_and(|j| j.starts_with(&[0xff, 0xd8])));
        assert_eq!(lightcraft_raw::decode(&s).is_ok(), uncompressed);
    }
}
