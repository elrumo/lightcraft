//! HEIF / HEIC and AVIF (ISO/IEC 23008-12, on the ISO base media file format of ISO/IEC 14496-12):
//! the metadata items of an iPhone photo. The top-level `meta` box lists the file's items in
//! `iinf` (each `infe` gives an item's id and type) and where their bytes are in `iloc` (extents in
//! the file, or in the `meta` box's own `idat`). The `Exif` item is a 4-byte offset to the TIFF
//! header followed by the EXIF block; XMP is a `mime` item of type `application/rdf+xml`.
//!
//! Every size and offset is checked against the buffer; box, item and extent counts and the
//! bytes gathered are bounded.

/// The metadata blocks of a HEIF file; `None` when it is not one.
pub fn metadata(bytes: &[u8]) -> Option<(Option<Vec<u8>>, Option<String>)> {
    if !is_heif(bytes) {
        return None;
    }
    let mut budget = MAX_BOXES;
    let top = boxes(bytes, 0, bytes.len(), &mut budget);
    let Some(meta) = top.iter().find(|b| &b.kind == b"meta") else { return Some((None, None)) };
    // `meta` is a full box: version and flags before its children
    let children = boxes(bytes, meta.body.saturating_add(4), meta.end, &mut budget);
    let items = children.iter().find(|b| &b.kind == b"iinf").map(|b| item_infos(bytes, b, &mut budget)).unwrap_or_default();
    let locations = children.iter().find(|b| &b.kind == b"iloc").map(|b| item_locations(bytes, b)).unwrap_or_default();
    let idat = children.iter().find(|b| &b.kind == b"idat").and_then(|b| bytes.get(b.body..b.end));
    let data = |id: u32| locations.iter().find(|l| l.id == id).and_then(|l| gather(bytes, idat, l));
    let exif = items.iter().filter(|i| &i.kind == b"Exif").find_map(|i| data(i.id)).and_then(|d| {
        // a big-endian offset from after itself to the TIFF header (usually past "Exif\0\0")
        let skip = usize::try_from(u32::from_be_bytes(d.get(..4)?.try_into().ok()?)).ok()?;
        d.get(4usize.checked_add(skip)?..).filter(|t| t.starts_with(b"II") || t.starts_with(b"MM")).map(<[u8]>::to_vec)
    });
    let xmp = items
        .iter()
        .filter(|i| &i.kind == b"mime" && i.content_type.eq_ignore_ascii_case("application/rdf+xml"))
        .find_map(|i| data(i.id))
        .map(|d| String::from_utf8_lossy(&d).into_owned());
    Some((exif, xmp))
}

/// `ftyp` naming an image brand of HEIF (HEIC / HEIX, a generic image file, AVIF).
pub fn is_heif(bytes: &[u8]) -> bool {
    if bytes.get(4..8) != Some(b"ftyp".as_slice()) {
        return false;
    }
    let Some(size) = be32(bytes, 0).and_then(|s| usize::try_from(s).ok()).filter(|s| *s >= 16) else { return false };
    let Some(ftyp) = bytes.get(8..size.min(bytes.len()).min(256)) else { return false };
    const BRANDS: [&[u8; 4]; 8] = [b"heic", b"heix", b"heim", b"heis", b"mif1", b"msf1", b"avif", b"avis"];
    // the major brand, then the compatible brands after the minor version
    ftyp.chunks_exact(4).enumerate().filter(|(i, _)| *i != 1).any(|(_, b)| BRANDS.iter().any(|x| x.as_slice() == b))
}

const MAX_BOXES: usize = 4096;
const MAX_ITEMS: usize = 4096;
const MAX_EXTENTS: usize = 64;
/// Metadata items bigger than this are not read (EXIF is at most 64 KiB in practice; XMP a few).
const MAX_ITEM_BYTES: usize = 16 << 20;

struct BoxRef {
    kind: [u8; 4],
    body: usize,
    end: usize,
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
        let (len, hdr) = match size {
            0 => (to - at, 8),
            1 => match be64(bytes, at + 8).and_then(|v| usize::try_from(v).ok()) {
                Some(l) => (l, 16),
                None => break,
            },
            n => (n as usize, 8),
        };
        let Some(end) = at.checked_add(len).filter(|&e| e <= to && len >= hdr) else { break };
        out.push(BoxRef { kind: kind4, body: at + hdr, end });
        at = end;
    }
    out
}

struct ItemInfo {
    id: u32,
    kind: [u8; 4],
    content_type: String,
}

/// `iinf`: the `infe` entries (versions 2 and 3, the ones HEIF uses).
fn item_infos(bytes: &[u8], iinf: &BoxRef, budget: &mut usize) -> Vec<ItemInfo> {
    let Some(version) = bytes.get(iinf.body).copied() else { return Vec::new() };
    let first = iinf.body.saturating_add(if version == 0 { 6 } else { 8 });
    let mut out = Vec::new();
    for e in boxes(bytes, first, iinf.end, budget).into_iter().filter(|b| &b.kind == b"infe").take(MAX_ITEMS) {
        let Some(v) = bytes.get(e.body).copied() else { continue };
        let at = e.body + 4;
        let (id, at) = match v {
            2 => (be16(bytes, at).map(u32::from), at + 2),
            3 => (be32(bytes, at), at + 4),
            _ => continue,
        };
        let Some(id) = id else { continue };
        // item_protection_index, then the item type
        let Some(kind) = bytes.get(at + 2..at + 6) else { continue };
        let mut kind4 = [0u8; 4];
        kind4.copy_from_slice(kind);
        // item_name, then (for `mime`) content_type: NUL-terminated strings
        let rest = bytes.get(at + 6..e.end).unwrap_or(&[]);
        let mut strings = rest.split(|c| *c == 0);
        let _name = strings.next();
        let content_type =
            if &kind4 == b"mime" { strings.next().map(|s| String::from_utf8_lossy(s).into_owned()).unwrap_or_default() } else { String::new() };
        out.push(ItemInfo { id, kind: kind4, content_type });
    }
    out
}

struct ItemLocation {
    id: u32,
    /// 0: offsets in the file; 1: in `idat`; 2 (item references) isn't read.
    method: u8,
    extents: Vec<(u64, u64)>,
}

/// `iloc`: each item's extents.
fn item_locations(bytes: &[u8], iloc: &BoxRef) -> Vec<ItemLocation> {
    let mut r = Reader { bytes, at: iloc.body, end: iloc.end };
    let mut out = Vec::new();
    let Some(version) = r.u8() else { return out };
    if version > 2 || r.skip(3).is_none() {
        return out;
    }
    let (Some(a), Some(b)) = (r.u8(), r.u8()) else { return out };
    let (offset_size, length_size, base_size) = (a >> 4, a & 15, b >> 4);
    let index_size = if version >= 1 { b & 15 } else { 0 };
    let count = if version < 2 { r.uint(2) } else { r.uint(4) };
    let Some(count) = count else { return out };
    for _ in 0..count.min(MAX_ITEMS as u64) {
        let Some(id) = (if version < 2 { r.uint(2) } else { r.uint(4) }).and_then(|v| u32::try_from(v).ok()) else { break };
        let method = if version >= 1 { r.uint(2).map(|v| (v & 15) as u8) } else { Some(0) };
        let (Some(method), Some(_data_ref), Some(base)) = (method, r.uint(2), r.uint(base_size)) else { break };
        let Some(n) = r.uint(2) else { break };
        let mut extents = Vec::new();
        for _ in 0..n {
            if index_size > 0 && r.uint(index_size).is_none() {
                break;
            }
            let (Some(off), Some(len)) = (r.uint(offset_size), r.uint(length_size)) else { break };
            if extents.len() < MAX_EXTENTS {
                extents.push((base.saturating_add(off), len));
            }
        }
        out.push(ItemLocation { id, method, extents });
    }
    out
}

/// An item's bytes: its extents joined (a zero length means "to the end").
fn gather(bytes: &[u8], idat: Option<&[u8]>, l: &ItemLocation) -> Option<Vec<u8>> {
    let src = match l.method {
        0 => bytes,
        1 => idat?,
        _ => return None,
    };
    let mut out = Vec::new();
    for &(off, len) in &l.extents {
        let off = usize::try_from(off).ok()?;
        let end = if len == 0 { src.len() } else { off.checked_add(usize::try_from(len).ok()?)? };
        let part = src.get(off..end)?;
        if out.len().saturating_add(part.len()) > MAX_ITEM_BYTES {
            return None;
        }
        out.extend_from_slice(part);
    }
    (!out.is_empty()).then_some(out)
}

/// Big-endian fields of `iloc`, bounded by its box.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    end: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.end)?;
        let s = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }
    /// An unsigned integer of `n` bytes (0, 4 or 8 in `iloc`; 0 = absent, read as 0).
    fn uint(&mut self, n: u8) -> Option<u64> {
        let s = self.take(usize::from(n))?;
        if s.len() > 8 {
            return None;
        }
        Some(s.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b)))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exif::tests::sample_exif;
    use lightcraft_tiff::ByteOrder;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn full(kind: &[u8; 4], version: u8, body: &[u8]) -> Vec<u8> {
        let mut b = vec![version, 0, 0, 0];
        b.extend_from_slice(body);
        bx(kind, &b)
    }

    fn infe(id: u16, kind: &[u8; 4], extra: &[u8]) -> Vec<u8> {
        let mut b = id.to_be_bytes().to_vec();
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(kind);
        b.extend_from_slice(b"\0");
        b.extend_from_slice(extra);
        full(b"infe", 2, &b)
    }

    /// A HEIC shaped like an iPhone's: the Exif item in `mdat` (file offsets), XMP in `idat`.
    fn sample_heic() -> Vec<u8> {
        let mut exif_item = vec![0, 0, 0, 6];
        exif_item.extend_from_slice(b"Exif\0\0");
        exif_item.extend(sample_exif(ByteOrder::Big));
        let xmp = b"<x:xmpmeta xmlns:x='adobe:ns:meta/'><rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'/></x:xmpmeta>".to_vec();
        let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1heic");
        let mut iinf_body = 2u16.to_be_bytes().to_vec();
        iinf_body.extend(infe(1, b"Exif", b""));
        iinf_body.extend(infe(2, b"mime", b"application/rdf+xml\0"));
        let iinf = full(b"iinf", 0, &iinf_body);
        let idat = bx(b"idat", &xmp);
        // iloc v1: offset 4 bytes, length 4 bytes, base 0, index 0
        let iloc_len = 4 + 2 + 2 + 2 * (2 + 2 + 2 + 2 + 8);
        let meta_len = 8 + 4 + iinf.len() + (8 + iloc_len) + idat.len();
        let mdat_at = ftyp.len() + meta_len;
        let exif_at = mdat_at + 8;
        let mut iloc = vec![0x44, 0x00];
        iloc.extend_from_slice(&2u16.to_be_bytes());
        for (id, method, off, len) in [(1u16, 0u16, exif_at as u32, exif_item.len() as u32), (2, 1, 0, xmp.len() as u32)] {
            iloc.extend_from_slice(&id.to_be_bytes());
            iloc.extend_from_slice(&method.to_be_bytes());
            iloc.extend_from_slice(&0u16.to_be_bytes()); // data reference
            iloc.extend_from_slice(&1u16.to_be_bytes()); // one extent
            iloc.extend_from_slice(&off.to_be_bytes());
            iloc.extend_from_slice(&len.to_be_bytes());
        }
        let iloc = full(b"iloc", 1, &iloc);
        assert_eq!(iloc.len(), 8 + iloc_len);
        let mut meta_body = vec![0, 0, 0, 0];
        meta_body.extend(iinf);
        meta_body.extend(iloc);
        meta_body.extend(idat);
        let meta = bx(b"meta", &meta_body);
        let mut f = ftyp;
        f.extend(meta);
        assert_eq!(f.len(), mdat_at);
        f.extend(bx(b"mdat", &exif_item));
        f
    }

    #[test]
    fn heic_exif_and_xmp_are_found() {
        let f = sample_heic();
        assert!(is_heif(&f));
        let (exif, xmp) = metadata(&f).unwrap();
        assert!(exif.as_deref().is_some_and(|t| t.starts_with(b"MM")), "{exif:?}");
        assert!(xmp.is_some_and(|x| x.contains("xmpmeta")));
        // through the crate's reader: capture time, camera, orientation
        let m = crate::extract(&f);
        assert_eq!(m.model.as_deref(), Some("Model X"));
        assert_eq!(m.orientation, Some(lightcraft_geom::Orientation::from_exif(6)));
        assert!(m.capture_time.is_some(), "{m:?}");
    }

    #[test]
    fn other_files_and_damage_are_handled() {
        assert!(!is_heif(b"\0\0\0\x18ftypcrx \0\0\0\x01crx isom"));
        assert!(!is_heif(b"\xff\xd8\xff"));
        assert!(metadata(b"").is_none());
        let f = sample_heic();
        // every truncation and a corrupted byte at every position: no panic, no out-of-range read
        for cut in 0..f.len() {
            let _ = metadata(&f[..cut]);
        }
        for i in 0..f.len() {
            let mut g = f.clone();
            g[i] ^= 0xa5;
            let _ = metadata(&g);
            g[i] = 0xff;
            let _ = metadata(&g);
        }
    }
}
