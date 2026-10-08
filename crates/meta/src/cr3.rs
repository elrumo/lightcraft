//! Canon CR3: the ISO base media file container (ISO/IEC 14496-12 box structure; CR3 layout from Laurent
//! Clévy's public CR3 format notes, confirmed black-box on EOS R6 Mark III files).
//!
//! - `ftyp` with major brand `crx `.
//! - `moov` holds a Canon `uuid` box (85c0b687-820f-11e0-8111-f4ce462b6a48) whose children are `CMT1` (a TIFF
//!   stream: IFD0), `CMT2` (TIFF: the Exif IFD), `CMT3` (TIFF: the Canon maker note), `CMT4` (TIFF: GPS) and the
//!   `THMB` thumbnail, then one `trak` per stream: a full-size JPEG, a reduced raw, the full raw (`CRAW` sample
//!   entries; raws carry a `CMP1` coding header and a `CDI1`/`IAD1` image-area box) and timed metadata (`CTMD`).
//! - A top-level `uuid` box with the XMP uuid (be7acfcb-97a9-42e8-9c71-999491e3afac, XMP spec part 3) holds the
//!   XMP packet.
//!
//! Every size and offset is checked against the buffer; nesting and box counts are bounded.

/// A track's sample-entry kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cr3TrackKind {
    /// `CRAW` with a `JPEG` child: the full-size JPEG.
    Jpeg,
    /// `CRAW` raw image: dimensions, the `CMP1` coding header and the `CDI1`/`IAD1` image-area box.
    Raw { width: u16, height: u16, cmp1: Option<Cmp1>, iad1: Option<Iad1> },
    /// Anything else (`CTMD` timed metadata, unknown entries).
    Other([u8; 4]),
}

/// The `CMP1` coding header of a raw track: sizes of the image and its tiles, and how the planes are coded.
/// Field layout from Clévy's notes (credited there to Alexey Danilchenko); the nibble order of the packed bytes
/// was confirmed on an EOS M50 C-RAW file (planes in the high nibble, Bayer layout in the low one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cmp1 {
    /// 0x100 on current bodies.
    pub version: u16,
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    /// Bits per sample (14 on current bodies).
    pub bits: u8,
    /// Coded planes: 4 (R, G1, G2, B) for Bayer data.
    pub planes: u8,
    /// Bayer layout of the planes: 0 = RGGB, 1 = GRBG, 2 = GBRG, 3 = BGGR (meaningful with more than one plane).
    pub cfa_layout: u8,
    /// 0 for still raw (lossless and C-RAW), 3 for roll-burst.
    pub enc_type: u8,
    /// Wavelet decomposition levels: 0 for lossless, 3 for C-RAW.
    pub wavelet_levels: u8,
    pub tiles_across: bool,
    pub tiles_down: bool,
    /// Size of the header at the start of the sample (before the first tile's data).
    pub header_size: u32,
}

impl Cmp1 {
    /// Parse the payload of a `CMP1` box (offsets after the box header). `None` when it is too short.
    pub fn parse(b: &[u8]) -> Option<Cmp1> {
        let (planes_cfa, enc_levels, tiles) = (*b.get(25)?, *b.get(26)?, *b.get(27)?);
        Some(Cmp1 {
            version: be16(b, 4)?,
            width: be32(b, 8)?,
            height: be32(b, 12)?,
            tile_width: be32(b, 16)?,
            tile_height: be32(b, 20)?,
            bits: *b.get(24)?,
            planes: planes_cfa >> 4,
            cfa_layout: planes_cfa & 15,
            enc_type: enc_levels >> 4,
            wavelet_levels: enc_levels & 15,
            tiles_across: tiles & 0x80 != 0,
            tiles_down: tiles & 0x40 != 0,
            header_size: be32(b, 28)?,
        })
    }
}

/// An inclusive pixel rectangle as `IAD1` stores it: zero-based `left, top, right, bottom`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Iad1Rect {
    pub left: u16,
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
}

impl Iad1Rect {
    /// Width in pixels (the right edge is inclusive).
    pub fn width(&self) -> usize {
        (self.right as usize + 1).saturating_sub(self.left as usize)
    }
    pub fn height(&self) -> usize {
        (self.bottom as usize + 1).saturating_sub(self.top as usize)
    }
}

/// The four rectangles of a full-size raw's `IAD1` box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Iad1Areas {
    /// The recommended crop (the image the camera writes to its JPEG; the sensor-info borders of the maker note).
    pub crop: Iad1Rect,
    /// Optically black columns on the left.
    pub left_black: Iad1Rect,
    /// Optically black rows on top.
    pub top_black: Iad1Rect,
    /// The active area. Clévy notes it can overflow the image by a few pixels on some files; clamp it.
    pub active: Iad1Rect,
}

/// The `IAD1` image-area box inside `CDI1`: sensor size and, on full-size raws, the areas of the sensor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Iad1 {
    pub width: u16,
    pub height: u16,
    /// `None` on the reduced raw (its 32-byte box has no named areas).
    pub areas: Option<Iad1Areas>,
}

impl Iad1 {
    /// Parse the payload of an `IAD1` box (a full box: 4 bytes of version and flags, then big-endian `u16`s).
    pub fn parse(b: &[u8]) -> Option<Iad1> {
        let rect = |at: usize| Some(Iad1Rect { left: be16(b, at)?, top: be16(b, at + 2)?, right: be16(b, at + 4)?, bottom: be16(b, at + 6)? });
        let areas = rect(16).zip(rect(24)).zip(rect(32)).zip(rect(40)).map(|(((crop, left_black), top_black), active)| Iad1Areas {
            crop,
            left_black,
            top_black,
            active,
        });
        // the reduced raw's box ends after two unnamed groups of four values, so only the full-size one has areas
        Some(Iad1 { width: be16(b, 4)?, height: be16(b, 6)?, areas })
    }
}

/// One track's single sample: where it is in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cr3Track {
    pub kind: Cr3TrackKind,
    /// Byte offset and length of the sample (the first one) in the file, when the tables give them.
    pub data: Option<(usize, usize)>,
}

/// The parts of a CR3 file LightCraft reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cr3<'a> {
    /// `CMT1`…`CMT4` (index 0…3).
    pub cmt: [Option<&'a [u8]>; 4],
    pub thumbnail: Option<&'a [u8]>,
    pub xmp: Option<&'a [u8]>,
    pub tracks: Vec<Cr3Track>,
}

impl Cr3<'_> {
    /// The full-size raw track: the largest `CRAW` raw entry whose coding header and sample location were found
    /// (the reduced raw is also a `CRAW` raw entry).
    pub fn raw_track(&self) -> Option<&Cr3Track> {
        self.tracks.iter().filter(|t| t.data.is_some() && matches!(t.kind, Cr3TrackKind::Raw { cmp1: Some(_), .. })).max_by_key(|t| match t.kind {
            Cr3TrackKind::Raw { width, height, .. } => width as u32 * height as u32,
            _ => 0,
        })
    }
}

const CANON_UUID: [u8; 16] = [0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48];
const XMP_UUID: [u8; 16] = [0xbe, 0x7a, 0xcf, 0xcb, 0x97, 0xa9, 0x42, 0xe8, 0x9c, 0x71, 0x99, 0x94, 0x91, 0xe3, 0xaf, 0xac];
const MAX_DEPTH: usize = 12;
const MAX_BOXES: usize = 4096;
/// A `VisualSampleEntry` (78 bytes after the box header) plus 4 bytes Canon adds before the child boxes.
const CRAW_CHILDREN_AT: usize = 82;

/// Does this look like a CR3 file (`ftyp` box with major brand `crx `)?
pub fn is_cr3(bytes: &[u8]) -> bool {
    bytes.get(4..12) == Some(b"ftypcrx ".as_slice())
}

/// Parse a CR3 file's boxes; `None` when it is not a CR3 file.
pub fn parse_cr3(bytes: &[u8]) -> Option<Cr3<'_>> {
    if !is_cr3(bytes) {
        return None;
    }
    let mut out = Cr3::default();
    let mut budget = MAX_BOXES;
    for b in boxes(bytes, 0, bytes.len(), &mut budget) {
        match &b.kind {
            b"moov" => moov(bytes, &b, &mut out, &mut budget),
            b"uuid" if b.uuid == Some(XMP_UUID) => out.xmp = bytes.get(b.body..b.end),
            _ => {}
        }
    }
    Some(out)
}

struct BoxRef {
    kind: [u8; 4],
    /// Start of the payload (after the header and, for `uuid`, the 16-byte uuid).
    body: usize,
    end: usize,
    uuid: Option<[u8; 16]>,
}

/// The boxes directly inside `[from, to)`; stops at the first malformed header.
fn boxes(bytes: &[u8], from: usize, to: usize, budget: &mut usize) -> Vec<BoxRef> {
    let mut out = Vec::new();
    let mut at = from;
    let to = to.min(bytes.len());
    while at.saturating_add(8) <= to && *budget > 0 {
        *budget -= 1;
        let (Some(size), Some(kind)) = (be32(bytes, at), bytes.get(at + 4..at + 8)) else { break };
        let mut kind4 = [0u8; 4];
        kind4.copy_from_slice(kind);
        let (len, mut hdr) = match size {
            0 => (to - at, 8),
            1 => match be64(bytes, at + 8).and_then(|v| usize::try_from(v).ok()) {
                Some(l) => (l, 16),
                None => break,
            },
            n => (n as usize, 8),
        };
        let Some(end) = at.checked_add(len).filter(|&e| e <= to && len >= hdr) else { break };
        let mut uuid = None;
        if &kind4 == b"uuid" {
            let Some(u) = bytes.get(at + hdr..at + hdr + 16).filter(|_| at + hdr + 16 <= end) else { break };
            let mut a = [0u8; 16];
            a.copy_from_slice(u);
            uuid = Some(a);
            hdr += 16;
        }
        out.push(BoxRef { kind: kind4, body: at + hdr, end, uuid });
        at = end;
    }
    out
}

fn moov<'a>(bytes: &'a [u8], m: &BoxRef, out: &mut Cr3<'a>, budget: &mut usize) {
    for b in boxes(bytes, m.body, m.end, budget) {
        match &b.kind {
            b"uuid" if b.uuid == Some(CANON_UUID) => {
                for c in boxes(bytes, b.body, b.end, budget) {
                    let slot = match &c.kind {
                        b"CMT1" => 0,
                        b"CMT2" => 1,
                        b"CMT3" => 2,
                        b"CMT4" => 3,
                        b"THMB" => {
                            out.thumbnail = bytes.get(c.body..c.end);
                            continue;
                        }
                        _ => continue,
                    };
                    out.cmt[slot] = bytes.get(c.body..c.end);
                }
            }
            b"trak" => {
                if let Some(t) = trak(bytes, &b, budget) {
                    out.tracks.push(t);
                }
            }
            _ => {}
        }
    }
}

/// Descend `trak` → `mdia` → `minf` → `stbl` and read its sample entry and first sample's location.
fn trak(bytes: &[u8], t: &BoxRef, budget: &mut usize) -> Option<Cr3Track> {
    let mut stbl = None;
    let mut stack = vec![(t.body, t.end, 0usize)];
    while let Some((from, to, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        for b in boxes(bytes, from, to, budget) {
            match &b.kind {
                b"mdia" | b"minf" => stack.push((b.body, b.end, depth + 1)),
                b"stbl" => stbl = Some(b),
                _ => {}
            }
        }
    }
    let stbl = stbl?;
    let (mut kind, mut size, mut offset) = (None, None, None);
    for b in boxes(bytes, stbl.body, stbl.end, budget) {
        match &b.kind {
            // full box: version/flags, entry count, entries
            b"stsd" => kind = boxes(bytes, b.body + 8, b.end, budget).first().map(|e| sample_entry(bytes, e, budget)),
            // version/flags, sample_size (0 = per-sample table), count, sizes
            b"stsz" => {
                size = match be32(bytes, b.body + 4)? {
                    0 if be32(bytes, b.body + 8)? > 0 => be32(bytes, b.body + 12),
                    0 => None,
                    n => Some(n),
                }
            }
            b"co64" if be32(bytes, b.body + 4)? > 0 => offset = be64(bytes, b.body + 8).and_then(|v| usize::try_from(v).ok()),
            b"stco" if be32(bytes, b.body + 4)? > 0 => offset = be32(bytes, b.body + 8).map(|v| v as usize),
            _ => {}
        }
    }
    let data = match (offset, size) {
        (Some(o), Some(s)) if o.checked_add(s as usize).is_some_and(|e| e <= bytes.len()) => Some((o, s as usize)),
        _ => None,
    };
    Some(Cr3Track { kind: kind?, data })
}

fn sample_entry(bytes: &[u8], e: &BoxRef, budget: &mut usize) -> Cr3TrackKind {
    if &e.kind != b"CRAW" {
        return Cr3TrackKind::Other(e.kind);
    }
    let (Some(width), Some(height)) = (be16(bytes, e.body + 24), be16(bytes, e.body + 26)) else {
        return Cr3TrackKind::Other(e.kind);
    };
    let (mut cmp1, mut iad1) = (None, None);
    for c in boxes(bytes, e.body.saturating_add(CRAW_CHILDREN_AT), e.end, budget) {
        match &c.kind {
            b"JPEG" => return Cr3TrackKind::Jpeg,
            b"CMP1" => cmp1 = bytes.get(c.body..c.end).and_then(Cmp1::parse),
            // full box (version and flags), then the `IAD1` box
            b"CDI1" => {
                iad1 = boxes(bytes, c.body.saturating_add(4), c.end, budget)
                    .iter()
                    .find(|i| &i.kind == b"IAD1")
                    .and_then(|i| bytes.get(i.body..i.end))
                    .and_then(Iad1::parse)
            }
            _ => {}
        }
    }
    Cr3TrackKind::Raw { width, height, cmp1, iad1 }
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at.checked_add(2)?).map(|s| u16::from_be_bytes([s[0], s[1]]))
}
fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at.checked_add(4)?).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}
fn be64(b: &[u8], at: usize) -> Option<u64> {
    let s = b.get(at..at.checked_add(8)?)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Some(u64::from_be_bytes(a))
}

/// Merge `CMT1` (IFD0), `CMT2` (Exif) and `CMT4` (GPS) into one TIFF stream, so the Exif readers see a CR3
/// like any TIFF-based raw. The maker note (`CMT3`) is left out: its offsets are relative to its own block.
pub fn merged_exif(c: &Cr3<'_>) -> Option<Vec<u8>> {
    use lightcraft_tiff::{IfdBuilder, Tiff, TiffWriter, tags as t};
    let ifd0 = Tiff::parse(c.cmt[0]?).ok()?;
    let order = ifd0.order;
    let first = |i: usize| c.cmt[i].and_then(|b| Tiff::parse(b).ok()).and_then(|t| t.ifds.into_iter().next());
    // pointers and offsets that would dangle once the IFDs move
    const SKIP: [u16; 9] = [t::EXIF_IFD, t::GPS_IFD, t::INTEROP_IFD, t::SUB_IFDS, t::MAKER_NOTE, 273, 279, 513, 514];
    let build = |ifd: &lightcraft_tiff::Ifd| {
        let mut b = IfdBuilder::new();
        for e in ifd.entries.iter().filter(|e| !SKIP.contains(&e.tag)) {
            b.set(e.tag, e.value.clone());
        }
        b
    };
    let mut root = build(ifd0.ifds.first()?);
    if let Some(exif) = first(1) {
        root.set_child(t::EXIF_IFD, build(&exif));
    }
    if let Some(gps) = first(3) {
        root.set_child(t::GPS_IFD, build(&gps));
    }
    TiffWriter::new(order, false).write(&[root]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }
    fn uuid(u: &[u8; 16], body: &[u8]) -> Vec<u8> {
        let mut b = u.to_vec();
        b.extend_from_slice(body);
        bx(b"uuid", &b)
    }
    fn full(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 4];
        b.extend_from_slice(body);
        bx(kind, &b)
    }
    fn craw(w: u16, h: u16, children: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; CRAW_CHILDREN_AT];
        b[24..26].copy_from_slice(&w.to_be_bytes());
        b[26..28].copy_from_slice(&h.to_be_bytes());
        b.extend_from_slice(children);
        bx(b"CRAW", &b)
    }
    fn trak(entry: Vec<u8>, offset: u64, size: u32) -> Vec<u8> {
        let mut stsd = 1u32.to_be_bytes().to_vec();
        stsd.extend(entry);
        let mut stsz = 0u32.to_be_bytes().to_vec();
        stsz.extend(1u32.to_be_bytes());
        stsz.extend(size.to_be_bytes());
        let mut co64 = 1u32.to_be_bytes().to_vec();
        co64.extend(offset.to_be_bytes());
        let stbl = [full(b"stsd", &stsd), full(b"stsz", &stsz), full(b"co64", &co64)].concat();
        bx(b"trak", &bx(b"mdia", &bx(b"minf", &bx(b"stbl", &stbl))))
    }
    /// A `CMP1` payload: 14-bit, four planes (RGGB), three wavelet levels, tiles side by side.
    fn cmp1_body(w: u32, h: u32, tile_w: u32, tile_h: u32) -> Vec<u8> {
        let mut b = vec![0xff, 0, 0, 0x30, 1, 0, 0, 0];
        for v in [w, h, tile_w, tile_h] {
            b.extend(v.to_be_bytes());
        }
        b.extend([14, 0x40, 0x03, 0x80, 0, 0, 0x04, 0x38, 0, 0, 0, 0]);
        b.extend([1, 1, 0, 0].repeat(4));
        b
    }
    /// An `IAD1` payload inside `CDI1`: version/flags, size, four header values, then `rects` (inclusive rectangles).
    fn cdi1(w: u16, h: u16, rects: &[[u16; 4]]) -> Vec<u8> {
        let mut b = vec![0u8; 4];
        for v in [w, h, 1, if rects.len() > 2 { 2 } else { 0 }, 1, 0] {
            b.extend(v.to_be_bytes());
        }
        for v in rects.concat() {
            b.extend(v.to_be_bytes());
        }
        let iad1 = bx(b"IAD1", &b);
        full(b"CDI1", &iad1)
    }
    fn tiff_with(tag: u16, v: lightcraft_tiff::Value) -> Vec<u8> {
        let b = lightcraft_tiff::IfdBuilder::new().with(tag, v);
        lightcraft_tiff::TiffWriter::new(lightcraft_tiff::ByteOrder::Little, false).write(&[b]).unwrap()
    }

    /// A small synthetic CR3: Canon uuid with CMT1/CMT2/CMT4, a JPEG track, a raw track, XMP.
    pub(crate) fn sample() -> Vec<u8> {
        use lightcraft_tiff::{Value, tags as t};
        let cmt1 = tiff_with(t::MODEL, Value::Ascii("Canon EOS Test".into()));
        let cmt2 = tiff_with(t::ISO_SPEED, Value::Short(vec![400]));
        let cmt4 = tiff_with(1, Value::Ascii("N".into()));
        let canon = uuid(&CANON_UUID, &[bx(b"CMT1", &cmt1), bx(b"CMT2", &cmt2), bx(b"CMT4", &cmt4), bx(b"THMB", b"thumb")].concat());
        let mut file = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
        let payload_at = 4096u64;
        let jpeg_t = trak(craw(64, 48, &bx(b"JPEG", &[0; 4])), payload_at, 16);
        // the reduced raw comes first, as in real files; its IAD1 has no named areas
        let small_children = [bx(b"CMP1", &cmp1_body(1624, 1080, 1624, 1080)), cdi1(1624, 1080, &[[1, 0, 1620, 1079], [0, 0, 1623, 1079]])].concat();
        let small_t = trak(craw(1624, 1080, &small_children), payload_at + 16, 16);
        // full size: crop, left black, top black and active rectangles, as on an EOS M50 (6288 × 4056, 6000 × 4000 image)
        let areas = [[276, 48, 6275, 4047], [0, 0, 263, 4055], [264, 0, 6287, 35], [264, 36, 6287, 4055]];
        let raw_children = [bx(b"CMP1", &cmp1_body(6288, 4056, 3144, 4056)), cdi1(6288, 4056, &areas)].concat();
        let raw_t = trak(craw(6288, 4056, &raw_children), payload_at + 32, 32);
        file.extend(bx(b"moov", &[canon, jpeg_t, small_t, raw_t].concat()));
        file.extend(uuid(&XMP_UUID, b"<x:xmpmeta/>"));
        file.resize(payload_at as usize + 64, 0);
        file
    }

    #[test]
    fn parses_canon_boxes_tracks_and_xmp() {
        let f = sample();
        assert!(is_cr3(&f));
        let c = parse_cr3(&f).unwrap();
        assert!(c.cmt[0].is_some() && c.cmt[1].is_some() && c.cmt[2].is_none() && c.cmt[3].is_some());
        assert_eq!(c.thumbnail, Some(b"thumb".as_slice()));
        assert_eq!(c.xmp, Some(b"<x:xmpmeta/>".as_slice()));
        assert_eq!(c.tracks.len(), 3);
        assert_eq!(c.tracks[0], Cr3Track { kind: Cr3TrackKind::Jpeg, data: Some((4096, 16)) });
        assert_eq!(c.tracks[1].data, Some((4112, 16)));
        assert_eq!(c.tracks[2].data, Some((4128, 32)));
    }

    #[test]
    fn reads_coding_header_and_image_areas_of_the_full_size_raw() {
        let f = sample();
        let c = parse_cr3(&f).unwrap();
        // the reduced raw is listed first; the full-size one is the largest
        let full = c.raw_track().unwrap();
        assert_eq!(full.data, Some((4128, 32)));
        let Cr3TrackKind::Raw { width, height, cmp1: Some(h), iad1: Some(a) } = full.kind else { panic!("{:?}", full.kind) };
        assert_eq!((width, height), (6288, 4056));
        assert_eq!(
            h,
            Cmp1 {
                version: 0x100,
                width: 6288,
                height: 4056,
                tile_width: 3144,
                tile_height: 4056,
                bits: 14,
                planes: 4,
                cfa_layout: 0,
                enc_type: 0,
                wavelet_levels: 3,
                tiles_across: true,
                tiles_down: false,
                header_size: 0x438,
            }
        );
        assert_eq!((a.width, a.height), (6288, 4056));
        let areas = a.areas.unwrap();
        assert_eq!((areas.crop.width(), areas.crop.height()), (6000, 4000));
        assert_eq!((areas.left_black.width(), areas.left_black.height()), (264, 4056));
        assert_eq!((areas.top_black.width(), areas.top_black.height()), (6024, 36));
        assert_eq!((areas.active.left, areas.active.top, areas.active.width(), areas.active.height()), (264, 36, 6024, 4020));
        // the reduced raw's IAD1 names no areas
        let Cr3TrackKind::Raw { iad1: Some(small), .. } = c.tracks[1].kind else { panic!("{:?}", c.tracks[1]) };
        assert_eq!((small.width, small.height, small.areas), (1624, 1080, None));
    }

    #[test]
    fn short_headers_are_not_parsed() {
        let body = cmp1_body(6288, 4056, 3144, 4056);
        assert!(Cmp1::parse(&body).is_some());
        assert!(Cmp1::parse(&body[..31]).is_none());
        assert!(Cmp1::parse(&[]).is_none());
        assert!(Iad1::parse(&[0; 7]).is_none());
        assert_eq!(Iad1::parse(&[0; 47]).unwrap().areas, None);
        // an inverted rectangle has no pixels instead of underflowing
        assert_eq!(Iad1Rect { left: 9, top: 9, right: 3, bottom: 3 }.width(), 0);
    }

    #[test]
    fn merged_exif_reads_like_a_tiff_raw() {
        let f = sample();
        let m = crate::read_exif(&merged_exif(&parse_cr3(&f).unwrap()).unwrap());
        assert_eq!(m.model.as_deref(), Some("Canon EOS Test"));
        assert_eq!(m.iso, Some(400));
    }

    #[test]
    fn hostile_input_never_panics() {
        let f = sample();
        // every truncation and single-byte corruption parses to something or nothing
        // (the box tree, including the coding headers, ends before the 4096-byte payloads)
        for n in 0..f.len().min(4096) {
            let _ = parse_cr3(&f[..n]).map(|c| (merged_exif(&c), c.raw_track().is_some()));
        }
        for i in 0..f.len().min(4096) {
            for v in [0u8, 1, 0x7f, 0xff] {
                let mut g = f.clone();
                g[i] = v;
                let _ = parse_cr3(&g).map(|c| (merged_exif(&c), c.raw_track().is_some()));
            }
        }
        // a sample offset past the end is not reported
        let mut g = f.clone();
        g.truncate(4100);
        assert!(parse_cr3(&g).unwrap().tracks.iter().all(|t| t.data.is_none()));
    }
}
