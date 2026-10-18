//! Describing a raw file without reading its pixel data.
//!
//! [`probe_info`](crate::probe_info) and `lightcraft_meta::extract` read only a raw file's
//! headers, but those are a few hundred kilobytes of a file of tens of megabytes: an import from a
//! slow drive spends nearly all its time reading samples nothing looks at. [`header_plan`] says
//! which byte ranges of a file those readers need. It is asked about a buffer of the file's full
//! length in which only the ranges in `loaded` hold the file's bytes (the rest is zeros); the caller
//! loads the ranges it names and asks again. Once it says [`HeaderPlan::Ready`], probing the buffer
//! gives what probing the whole file gives.
//!
//! Only bulk sample data is left out: the strips and tiles of each large TIFF image (keeping the
//! start of the first one, where the coding headers a header probe validates sit), a CR3's image
//! tracks (keeping the raw track's coding headers and quantization maps, and the timed-metadata
//! track where Canon puts the as-shot white balance), a RAF's CFA strip (keeping its compression
//! header) and a Panasonic RW2's raw data. A Sony ARW6's strip is not a prefix but the scattered
//! bytes its header probe validates (the tile table, the stream headers and index tables, the
//! residual stream's first half: see [`arw6_keep`]). Formats whose header probe reads samples (NEF:
//! masked columns; ORF: bit depth and CFA phase; PEF: dark borders; ERF and MEF) and anything else
//! get [`HeaderPlan::Whole`], as does a structure that is incomplete, inconsistent or keeps metadata
//! inside its sample data: the plan never guesses.

use std::collections::HashSet;

use lightcraft_meta::cr3::{Cr3TrackKind, XMP_UUID, is_cr3, parse_cr3};
use lightcraft_tiff::{ByteOrder, FieldType, Tiff, tags as t};

use crate::RawFormat;

/// Bytes kept at the start of a skipped block: the coding headers a header probe validates there
/// (lossless-JPEG frame headers, a RAF's compression header and stripe table) fit with room to spare.
const HEAD: u64 = 64 * 1024;
/// An image whose chunks hold fewer bytes than this is read (thumbnails, small previews).
const BULK: u64 = 256 * 1024;
/// An out-of-line TIFF value longer than this is bulk (a Photoshop file's layer data), not metadata:
/// [`image_plan`] leaves it unread.
const BIG_VALUE: u64 = 16 << 20;
/// The same bounds as `lightcraft_tiff`'s reader.
const MAX_IFDS: usize = 1024;
const MAX_ENTRIES: u64 = 8192;
const MAX_BOXES: usize = 4096;
/// Child IFDs listed by one `SubIFDs` entry.
const MAX_CHILDREN: u64 = 4096;
/// The RAF raw container's pointer to its raw IFD.
const RAF_RAW_IFD: u16 = 0xf000;

/// Byte ranges `[start, end)` of a file: sorted, disjoint and not touching.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ranges(Vec<(u64, u64)>);

impl Ranges {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `[start, end)` (nothing when it is empty).
    pub fn insert(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        let (mut start, mut end) = (start, end);
        let mut out = Vec::with_capacity(self.0.len() + 1);
        let mut placed = false;
        for &(a, b) in &self.0 {
            if b < start {
                out.push((a, b));
            } else if a > end {
                if !placed {
                    out.push((start, end));
                    placed = true;
                }
                out.push((a, b));
            } else {
                start = start.min(a);
                end = end.max(b);
            }
        }
        if !placed {
            out.push((start, end));
        }
        self.0 = out;
    }

    /// Add every `[start, end)` of `spans` in one sort and merge (empty ones add nothing).
    pub fn extend(&mut self, spans: impl IntoIterator<Item = (u64, u64)>) {
        let mut all: Vec<(u64, u64)> = std::mem::take(&mut self.0);
        all.extend(spans.into_iter().filter(|&(a, b)| a < b));
        all.sort_unstable();
        let mut out: Vec<(u64, u64)> = Vec::with_capacity(all.len());
        for (a, b) in all {
            match out.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => out.push((a, b)),
            }
        }
        self.0 = out;
    }

    /// Whether all of `[start, end)` is covered (an empty range always is).
    pub fn contains(&self, start: u64, end: u64) -> bool {
        start >= end || self.0.iter().any(|&(a, b)| a <= start && end <= b)
    }

    /// Whether some of `[start, end)` is covered.
    pub fn overlaps(&self, start: u64, end: u64) -> bool {
        start < end && self.0.iter().any(|&(a, b)| a < end && start < b)
    }

    pub fn spans(&self) -> &[(u64, u64)] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Bytes covered.
    pub fn total(&self) -> u64 {
        self.0.iter().fold(0u64, |n, &(a, b)| n.saturating_add(b - a))
    }

    /// The parts of `[start, end)` not covered.
    pub fn gaps(&self, start: u64, end: u64) -> Ranges {
        let mut out = Ranges::new();
        let mut at = start;
        for &(a, b) in &self.0 {
            if b <= at {
                continue;
            }
            if a >= end {
                break;
            }
            if a > at {
                out.insert(at, a);
            }
            at = at.max(b);
        }
        if at < end {
            out.insert(at, end);
        }
        out
    }

    /// These ranges without the ones in `other`.
    pub fn minus(&self, other: &Ranges) -> Ranges {
        let mut out = Ranges::new();
        for &(a, b) in &self.0 {
            for &(x, y) in other.gaps(a, b).spans() {
                out.insert(x, y);
            }
        }
        out
    }
}

/// What [`header_plan`] says about a partly loaded file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderPlan {
    /// Load these ranges (none of them loaded yet) and ask again.
    Need(Ranges),
    /// Everything the header readers look at is loaded: probing the buffer gives the whole file's
    /// result.
    Ready,
    /// The file can't be described from part of it (why): read it whole.
    Whole(&'static str),
}

/// What of the raw file in `bytes` a header probe reads. `bytes` has the file's full length;
/// only the ranges in `loaded` hold its bytes (see the module docs).
pub fn header_plan(bytes: &[u8], loaded: &Ranges) -> HeaderPlan {
    plan(bytes, loaded, false)
}

/// [`header_plan`] for a raw shown from its embedded preview (a coding not decoded yet): also what
/// [`crate::embedded_preview`] reads — a CR3's JPEG track and preview boxes, the strips of a TIFF
/// raw's preview images.
pub fn preview_plan(bytes: &[u8], loaded: &Ranges) -> HeaderPlan {
    plan(bytes, loaded, true)
}

fn plan(bytes: &[u8], loaded: &Ranges, preview: bool) -> HeaderPlan {
    let mut w = Want::new(bytes, loaded);
    if !w.want(0, 16) {
        return w.pending_plan().unwrap_or(HeaderPlan::Whole("file too short"));
    }
    if is_cr3(bytes) {
        cr3_plan(bytes, loaded, preview)
    } else if bytes.starts_with(b"FUJIFILMCCD-RAW") {
        // (its preview, the embedded JPEG, is read anyway: its metadata are there)
        raf_plan(bytes, loaded)
    } else if bytes.get(..4).is_some_and(|m| matches!(m, b"IIRO" | b"IIRS" | b"MMOR")) {
        HeaderPlan::Whole("an ORF header probe reads samples")
    } else if Tiff::sniff(bytes).is_some() {
        tiff_plan(bytes, loaded, preview)
    } else {
        HeaderPlan::Whole("not a raw layout read in part")
    }
}

/// Collects the ranges a plan still needs.
struct Want<'a> {
    len: u64,
    loaded: &'a Ranges,
    need: Ranges,
    /// Out-of-line values past [`BIG_VALUE`], as [`walk_tiff`] met them.
    big: Ranges,
}

impl<'a> Want<'a> {
    fn new(bytes: &[u8], loaded: &'a Ranges) -> Self {
        Self { len: bytes.len() as u64, loaded, need: Ranges::new(), big: Ranges::new() }
    }

    /// Whether `[start, end)` (clipped to the file) is loaded; asks for what isn't.
    fn want(&mut self, start: u64, end: u64) -> bool {
        let end = end.min(self.len);
        if self.loaded.contains(start, end) {
            return true;
        }
        for &(a, b) in self.loaded.gaps(start, end).spans() {
            self.need.insert(a, b);
        }
        false
    }

    /// `Need` when something is missing (the asks are taken).
    fn pending_plan(&mut self) -> Option<HeaderPlan> {
        (!self.need.is_empty()).then(|| HeaderPlan::Need(std::mem::take(&mut self.need)))
    }
}

/// Everything but `holes` is needed. `meta` (IFD tables and out-of-line values) must not lie in a
/// hole; once all is loaded, `ready` decides.
fn finish(len: u64, loaded: &Ranges, holes: &Ranges, meta: &Ranges, ready: impl FnOnce() -> HeaderPlan) -> HeaderPlan {
    if holes.spans().iter().any(|&(a, b)| meta.overlaps(a, b)) {
        return HeaderPlan::Whole("metadata inside sample data");
    }
    let missing = holes.gaps(0, len).minus(loaded);
    if missing.is_empty() { ready() } else { HeaderPlan::Need(missing) }
}

fn tiff_plan(bytes: &[u8], loaded: &Ranges, preview: bool) -> HeaderPlan {
    let len = bytes.len() as u64;
    let mut w = Want::new(bytes, loaded);
    let meta = match walk_tiff(&mut w, 0, bytes, &[]) {
        Ok(meta) => meta,
        Err(why) => return HeaderPlan::Whole(why),
    };
    if let Some(plan) = w.pending_plan() {
        return plan;
    }
    // (the Make is usually in the first kilobytes: a NEF, PEF or ORF is known before the rest is read)
    if matches!(crate::probe(bytes), Some(RawFormat::Nef | RawFormat::Nrw | RawFormat::Orf | RawFormat::Pef | RawFormat::Erf | RawFormat::Mef)) {
        return HeaderPlan::Whole("its header probe reads samples");
    }
    let Ok(tiff) = Tiff::parse(bytes) else { return HeaderPlan::Whole("TIFF structure") };
    let mut holes = Ranges::new();
    let mut spans = Vec::new();
    for ifd in tiff.all_ifds() {
        let info = ifd.image().ok();
        // A JPEG-interchange image (compression 6/7: tags 0x0201/0x0202 are `JPEGInterchangeFormat` and
        // `JPEGInterchangeFormatLength`, not strip tags, which is why `image()` declines; e.g. a Sony
        // ARW's full-size JPEG): the header probe never reads it and `embedded_preview` wants it whole,
        // so under [`header_plan`] its bulk is holed like any other image's.
        let jif = (|| {
            if preview || !matches!(ifd.u16(t::COMPRESSION), Some(6 | 7)) {
                return None;
            }
            let off = ifd.u64(t::JPEG_INTERCHANGE_FORMAT)?;
            let jif_len = ifd.u64(t::JPEG_INTERCHANGE_FORMAT_LENGTH)?;
            (jif_len >= BULK).then_some((off, jif_len))
        })();
        let chunks: Vec<(u64, u64)> = match &info {
            // a preview's strips are read when the preview is wanted: only the raw image is skipped
            Some(info) if preview && !matches!(info.photometric, t::photometric::CFA | t::photometric::LINEAR_RAW) => continue,
            Some(info) => info.chunks(len).iter().map(|c| (c.offset, c.len)).collect(),
            // a raw IFD without the tags a TIFF image needs (an older CR2's) still lists its strips
            None => match (ifd.u64s(t::STRIP_OFFSETS), ifd.u64s(t::STRIP_BYTE_COUNTS)) {
                (Some(offsets), Some(counts)) if offsets.len() == counts.len() => offsets.into_iter().zip(counts).collect(),
                // nothing to hole but for a JPEG-interchange image (see `jif`)
                _ => {
                    if let Some((off, jif_len)) = jif {
                        holes.insert(off.saturating_add(HEAD), off.saturating_add(jif_len).min(len));
                    }
                    continue;
                }
            },
        };
        let total = chunks.iter().fold(0u64, |n, c| n.saturating_add(c.1));
        if total < BULK {
            continue;
        }
        // A compression-32766 (ARW6) strip: its header probe reads scattered bytes (the tile table, stream
        // headers, index tables, the residual's first half), not a prefix — keep exactly those and hole the
        // rest ([`arw6_keep`]). Multiple strips or a layout it cannot walk fail the probe identically on the
        // whole file, so they stay here for the generic prefix hole or fall to `Whole`.
        if let Some(info) = &info
            && info.compression == crate::vendor::arw6::COMPRESSION
            && let Some(&(chunk_offset, chunk_size)) = chunks.first()
        {
            match arw6_keep(&mut w, bytes, (chunk_offset, chunk_size), info.width as usize, info.height as usize) {
                None => return w.pending_plan().unwrap_or(HeaderPlan::Whole("ARW6 strip")),
                Some(Ok(keep)) => {
                    let mut full = Ranges::new();
                    full.insert(chunk_offset, chunk_offset.saturating_add(chunk_size).min(len));
                    for &(a, b) in full.minus(&keep).spans() {
                        holes.insert(a, b);
                    }
                    continue;
                }
                Some(Err(_)) => return HeaderPlan::Whole("ARW6 strip"),
            }
        }
        spans.extend(chunks.iter().enumerate().map(|(i, &(offset, size))| {
            let start = if i == 0 { offset.saturating_add(HEAD) } else { offset };
            (start, offset.saturating_add(size).min(len))
        }));
    }
    holes.extend(spans);
    // A Panasonic RW2 keeps its raw data from `RawDataOffset` to the end, outside any TIFF image.
    if holes.is_empty()
        && bytes.starts_with(b"IIU\0")
        && let Some(off) = tiff.ifds.first().and_then(|i| i.u64(crate::vendor::rw2::RAW_OFFSET))
    {
        holes.insert(off.saturating_add(HEAD), len);
    }
    finish(len, loaded, &holes, &meta, || match crate::probe(bytes) {
        Some(RawFormat::Nef | RawFormat::Nrw | RawFormat::Orf | RawFormat::Pef | RawFormat::Erf | RawFormat::Mef) => {
            HeaderPlan::Whole("its header probe reads samples")
        }
        Some(f) if f.is_supported() => HeaderPlan::Ready,
        _ => HeaderPlan::Whole("not a supported raw"),
    })
}

/// What decoding and describing a TIFF that is not a raw reads: its IFD tables, its metadata values
/// (EXIF, XMP, ICC; not a value past [`BIG_VALUE`]) and the strips or tiles of its first image, which
/// is the picture. A Photoshop file's layers sit in one huge tag value and further images (not the
/// first IFD), so a file of several GB reads as a fraction of that. Same contract as [`header_plan`]:
/// once it says [`HeaderPlan::Ready`], `lightcraft_codecs::read_header` and `decode` of the buffer
/// give what they give for the whole file.
pub fn image_plan(bytes: &[u8], loaded: &Ranges) -> HeaderPlan {
    let len = bytes.len() as u64;
    let mut w = Want::new(bytes, loaded);
    if !w.want(0, 16) {
        return w.pending_plan().unwrap_or(HeaderPlan::Whole("file too short"));
    }
    if Tiff::sniff(bytes).is_none() {
        return HeaderPlan::Whole("not a TIFF");
    }
    let meta = match walk_tiff(&mut w, 0, bytes, &[]) {
        Ok(meta) => meta,
        Err(why) => return HeaderPlan::Whole(why),
    };
    let big = std::mem::take(&mut w.big);
    if let Some(plan) = w.pending_plan() {
        return plan;
    }
    let Ok(tiff) = Tiff::parse(bytes) else { return HeaderPlan::Whole("TIFF structure") };
    let Some(Ok(info)) = tiff.ifds.first().map(|ifd| ifd.image()) else { return HeaderPlan::Whole("no image in the first IFD") };
    let mut keep = meta.minus(&big);
    for c in info.chunks(len) {
        keep.insert(c.offset, c.offset.saturating_add(c.len).min(len));
    }
    let missing = keep.minus(loaded);
    if missing.is_empty() { HeaderPlan::Ready } else { HeaderPlan::Need(missing) }
}

/// Walk the IFDs of the TIFF in `data` (which starts at `origin` in the file) as `lightcraft_tiff`'s
/// reader does — the chain, `SubIFDs`, Exif, GPS and Interop, plus the `extra` pointer tags —
/// asking for every IFD table and for the values that locate IFDs and image chunks. Returns the IFD
/// tables and out-of-line values found (absolute ranges); incomplete while `w` needs more.
fn walk_tiff(w: &mut Want, origin: u64, data: &[u8], extra: &[u16]) -> Result<Ranges, &'static str> {
    let len = data.len() as u64;
    let at = |rel: u64| origin.saturating_add(rel);
    let mut meta = Ranges::new();
    if !w.want(origin, at(len.min(16))) {
        return Ok(meta);
    }
    let (order, big) = Tiff::sniff(data).ok_or("not a TIFF")?;
    let first = if big { order.read_u64(data, 8) } else { order.read_u32(data, 4).map(u64::from) }.ok_or("truncated TIFF header")?;
    meta.insert(origin, at(len.min(if big { 16 } else { 8 })));
    let (head, entry, inline) = if big { (8u64, 20u64, 8u64) } else { (2, 12, 4) };
    let offset_at = |pos: u64| -> Option<u64> {
        let pos = usize::try_from(pos).ok()?;
        if big { order.read_u64(data, pos) } else { order.read_u32(data, pos).map(u64::from) }
    };
    let mut stack = vec![first];
    let mut seen = HashSet::new();
    while let Some(off) = stack.pop() {
        if off == 0 || off >= len || seen.len() >= MAX_IFDS || !seen.insert(off) {
            continue;
        }
        if !w.want(at(off), at(off.saturating_add(head))) {
            continue;
        }
        let Ok(pos) = usize::try_from(off) else { continue };
        let n = if big { order.read_u64(data, pos) } else { order.read_u16(data, pos).map(u64::from) }.unwrap_or(0);
        let table = off.saturating_add(head);
        let n_read = n.min(len.saturating_sub(table) / entry).min(MAX_ENTRIES);
        if n_read == 0 {
            continue;
        }
        let table_end = table.saturating_add(n_read * entry);
        // the reader reads the next-IFD pointer only when every entry fits
        let complete = n_read == n;
        let end = if complete { table_end.saturating_add(inline).min(len) } else { table_end };
        if !w.want(at(off), at(end)) {
            continue;
        }
        meta.insert(at(off), at(end));
        for i in 0..n_read {
            let p = table + i * entry;
            let Ok(pu) = usize::try_from(p) else { continue };
            let (Some(tag), Some(typ)) = (order.read_u16(data, pu), order.read_u16(data, pu + 2).and_then(FieldType::from_u16)) else { continue };
            let count = if big { order.read_u64(data, pu + 4) } else { order.read_u32(data, pu + 4).map(u64::from) };
            let Some(count) = count else { continue };
            let field = p + if big { 12 } else { 8 };
            let Some(total) = count.checked_mul(typ.size() as u64) else { continue };
            let start = if total <= inline {
                field
            } else {
                match offset_at(field) {
                    Some(v) => v,
                    None => continue,
                }
            };
            let Some(value_end) = start.checked_add(total).filter(|&e| e <= len) else { continue };
            if total > inline {
                meta.insert(at(start), at(value_end));
                if total > BIG_VALUE {
                    w.big.insert(at(start), at(value_end));
                }
            }
            let child = matches!(tag, t::SUB_IFDS | t::EXIF_IFD | t::GPS_IFD | t::INTEROP_IFD) || extra.contains(&tag);
            let locates = child || matches!(tag, t::STRIP_OFFSETS | t::STRIP_BYTE_COUNTS | t::TILE_OFFSETS | t::TILE_BYTE_COUNTS);
            if !locates || !w.want(at(start), at(value_end)) || !child {
                continue;
            }
            // the reader follows every `SubIFDs` value and the first value of the other pointers
            let take = if tag == t::SUB_IFDS { count.min(MAX_CHILDREN) } else { count.min(1) };
            for k in 0..take {
                if let Some(v) = unsigned(data, order, typ, start + k * typ.size() as u64) {
                    stack.push(v);
                }
            }
        }
        if complete && let Some(next) = offset_at(table_end) {
            stack.push(next);
        }
    }
    Ok(meta)
}

/// One unsigned value of type `typ` at `pos`.
fn unsigned(data: &[u8], order: ByteOrder, typ: FieldType, pos: u64) -> Option<u64> {
    let pos = usize::try_from(pos).ok()?;
    match typ.size() {
        1 => data.get(pos).map(|&b| u64::from(b)),
        2 => order.read_u16(data, pos).map(u64::from),
        4 => order.read_u32(data, pos).map(u64::from),
        _ => order.read_u64(data, pos),
    }
}

fn be32(bytes: &[u8], at: u64) -> Option<u64> {
    let at = usize::try_from(at).ok()?;
    bytes.get(at..at.checked_add(4)?).map(|s| u64::from(u32::from_be_bytes([s[0], s[1], s[2], s[3]])))
}

fn be64(bytes: &[u8], at: u64) -> Option<u64> {
    let at = usize::try_from(at).ok()?;
    bytes.get(at..at.checked_add(8)?).map(|s| u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

/// A CR3 is read where `lightcraft_meta::cr3::parse_cr3` and the CR3 decoder look: the file type,
/// `moov` and the XMP `uuid` box, the first sample of each timed-metadata (`CTMD`) track, and the
/// main raw track's coding headers and quantization maps. Previews and image tracks are skipped,
/// unless `preview`: then every top-level box but `mdat` and the JPEG tracks are read too.
fn cr3_plan(bytes: &[u8], loaded: &Ranges, preview: bool) -> HeaderPlan {
    let len = bytes.len() as u64;
    let mut w = Want::new(bytes, loaded);
    // top-level boxes, walked as `parse_cr3` walks them (it stops at the first malformed header)
    let mut at = 0u64;
    let mut boxes = 0;
    while at.saturating_add(8) <= len && boxes < MAX_BOXES {
        boxes += 1;
        // size, type, a 64-bit size and a uuid
        if !w.want(at, at.saturating_add(32)) {
            break;
        }
        let Some(size) = be32(bytes, at) else { break };
        let (box_len, hdr) = match size {
            0 => (len - at, 8),
            1 => match be64(bytes, at + 8) {
                Some(l) => (l, 16),
                None => break,
            },
            n => (n, 8),
        };
        let Some(end) = at.checked_add(box_len).filter(|&e| e <= len && box_len >= hdr) else { break };
        let kind = bytes.get(at as usize + 4..at as usize + 8);
        let needed = match kind {
            Some(b"mdat") => false,
            Some(b"moov") => true,
            Some(b"uuid") => preview || bytes.get((at + hdr) as usize..(at + hdr + 16) as usize) == Some(XMP_UUID.as_slice()),
            _ => preview,
        };
        if needed {
            w.want(at, end);
        }
        at = end;
    }
    if let Some(plan) = w.pending_plan() {
        return plan;
    }
    let mut w = Want::new(bytes, loaded);
    let Some(cr3) = parse_cr3(bytes) else { return HeaderPlan::Whole("CR3 structure") };
    for track in &cr3.tracks {
        if (track.kind == Cr3TrackKind::Other(*b"CTMD") || preview && track.kind == Cr3TrackKind::Jpeg)
            && let Some((offset, size)) = track.data
        {
            w.want(offset as u64, offset.saturating_add(size) as u64);
        }
    }
    if let Some(track) = crate::vendor::cr3::main_track(&cr3.tracks)
        && let Some((offset, size)) = track.data
        && let Some(coding) = track.compression(bytes)
    {
        let o = offset as u64;
        if w.want(o, o.saturating_add(u64::from(coding.header_size)))
            && let Some(sample) = bytes.get(offset..offset.saturating_add(size))
            && let Ok(ranges) = crate::vendor::crx::header_ranges(sample, &coding)
        {
            for r in ranges {
                w.want(o.saturating_add(r.start as u64), o.saturating_add(r.end as u64));
            }
        }
    }
    w.pending_plan().unwrap_or(HeaderPlan::Ready)
}

/// A RAF is read whole but for its CFA strip: the header, the embedded JPEG (its metadata), the
/// record directory and the raw container's IFDs and compression header.
fn raf_plan(bytes: &[u8], loaded: &Ranges) -> HeaderPlan {
    let len = bytes.len() as u64;
    let mut w = Want::new(bytes, loaded);
    if !w.want(0, 160) {
        return w.pending_plan().unwrap_or(HeaderPlan::Whole("RAF header"));
    }
    let (Some(start), Some(size)) = (be32(bytes, 100), be32(bytes, 104)) else { return HeaderPlan::Whole("RAF header") };
    let Some(end) = start.checked_add(size).filter(|&e| start > 0 && size > 0 && e <= len) else { return HeaderPlan::Whole("RAF raw container") };
    let Some(container) = bytes.get(start as usize..end as usize) else { return HeaderPlan::Whole("RAF raw container") };
    let meta = match walk_tiff(&mut w, start, container, &[RAF_RAW_IFD]) {
        Ok(meta) => meta,
        Err(why) => return HeaderPlan::Whole(why),
    };
    if let Some(plan) = w.pending_plan() {
        return plan;
    }
    let Ok(ifd) = crate::vendor::raf::raw_ifd(container) else { return HeaderPlan::Whole("RAF raw IFD") };
    let (Some(off), Some(strip)) = (ifd.u64(0xf007), ifd.u64(0xf008)) else { return HeaderPlan::Whole("RAF raw strip") };
    let mut holes = Ranges::new();
    if strip >= BULK {
        let s = start.saturating_add(off);
        holes.insert(s.saturating_add(HEAD), s.saturating_add(strip).min(end));
    }
    finish(len, loaded, &holes, &meta, || HeaderPlan::Ready)
}

/// Ask for several ranges in one go, keeping the loaded ones; false when any is still missing.
fn load_all(w: &mut Want, keep: &mut Ranges, spans: &[(u64, u64)]) -> bool {
    let mut all = true;
    for &(start, end) in spans {
        let end = end.min(w.len);
        if w.want(start, end) {
            keep.insert(start, end);
        } else {
            all = false;
        }
    }
    all
}

/// A compression-32766 (ARW6) strip's byte ranges that [`crate::vendor::arw6::decode`] reads in header mode — *not*
/// a prefix like the other codecs (`docs/arw6-compression.md`): the tile table, and per tile the picture and group
/// tables, the 13 stream header words, every stream's index table and the residual stream's first half, whose
/// lines fix the first-half shift. The later ranges' positions are only known once earlier ones are loaded, so the
/// walk runs in stages that ask for what they can in one round (`None` while something is still missing; its asks
/// wait in `w`); `Err` when the layout cannot be walked at all — the probe then stops at the same validation of
/// loaded bytes on the whole file, so the caller reads the file whole.
fn arw6_keep(w: &mut Want, bytes: &[u8], strip: (u64, u64), width: usize, height: usize) -> Option<Result<Ranges, &'static str>> {
    use crate::vendor::arw6;
    let (offset, size) = strip;
    let strip_end = offset.saturating_add(size).min(bytes.len() as u64);
    // the tile table: a count and at most sixteen 24-byte entries
    if !w.want(offset, offset.saturating_add(8 + 24 * arw6::MAX_TILES as u64).min(strip_end)) {
        return None;
    }
    let (at, end) = match (usize::try_from(offset), usize::try_from(strip_end)) {
        (Ok(at), Ok(end)) if at <= end => (at, end),
        _ => return Some(Err("ARW6 strip")),
    };
    let Some(strip) = bytes.get(at..end) else { return Some(Err("ARW6 strip")) };
    let Ok(tiles) = arw6::parse_tile_table(strip, width, height) else { return Some(Err("ARW6 tile table")) };
    // each tile's bytes run to the next tile's offset (the last: the strip end), as `tile_bytes` does: the
    // `end_rel(i)` are positions in the strip and `abs` makes one absolute
    let end_rel = |i: usize| -> usize {
        match tiles.get(i + 1) {
            Some(next) => next.offset,
            None => strip.len(),
        }
    };
    let abs = |rel: usize| offset.saturating_add(rel as u64);
    // stage A: every tile's picture header (word 0) and group tables (words 3..8)
    let mut keep = Ranges::new();
    keep.insert(offset, offset.saturating_add(8 + 24 * tiles.len() as u64).min(strip_end));
    let tables: Vec<(u64, u64)> = tiles.iter().enumerate().map(|(i, entry)| (abs(entry.offset), abs((entry.offset + 128).min(end_rel(i))))).collect();
    if !load_all(w, &mut keep, &tables) {
        return None;
    }
    // stage B: the 13 stream header words, at positions the group tables alone imply
    let mut spans = Vec::with_capacity(tiles.len() * 13);
    for (i, entry) in tiles.iter().enumerate() {
        let Some(found) = strip.get(entry.offset..(entry.offset + 128).min(end_rel(i))).and_then(arw6::stream_header_positions) else {
            return Some(Err("ARW6 tile header"));
        };
        let start = abs(entry.offset);
        for p in found {
            let at = start.saturating_add(p as u64 * 16);
            spans.push((at, at.saturating_add(16)));
        }
    }
    if !load_all(w, &mut keep, &spans) {
        return None;
    }
    // stage C: with the header words loaded, the probe's own parse places each stream's index table
    let mut headers = Vec::with_capacity(tiles.len());
    let mut spans = Vec::with_capacity(tiles.len() * 13);
    for (i, entry) in tiles.iter().enumerate() {
        let Some(tile) = strip.get(entry.offset..end_rel(i)) else { return Some(Err("ARW6 tile header")) };
        let Ok(header) = arw6::parse_tile_header(tile, entry.width, entry.height) else { return Some(Err("ARW6 tile header")) };
        let start = abs(entry.offset);
        for st in &header.streams {
            let from = start.saturating_add((st.word as u64 + 1) * 16);
            spans.push((from, from.saturating_add(st.index_words as u64 * 16)));
        }
        headers.push(header);
    }
    if !load_all(w, &mut keep, &spans) {
        return None;
    }
    // stage D: each residual stream's first half — its first index entry (loaded with the index table) gives
    // the half's byte length, and [`arw6::first_half_shift`] decodes its lines for the anchor offset
    let mut spans = Vec::with_capacity(tiles.len());
    for (i, entry) in tiles.iter().enumerate() {
        let Some(tile) = strip.get(entry.offset..end_rel(i)) else { return Some(Err("ARW6 tile header")) };
        let Some(header) = headers.get(i) else { return Some(Err("ARW6 tile header")) };
        let Some(g4) = header.streams.get(12) else { return Some(Err("ARW6 tile header")) };
        let Some(a) = arw6::u16_be(tile, (g4.word + 1) * 16) else { return Some(Err("ARW6 residual half")) };
        let data = abs(entry.offset).saturating_add((g4.word as u64 + 1) * 16).saturating_add(g4.index_words as u64 * 16);
        spans.push((data, data.saturating_add(a as u64).min(abs(end_rel(i)))));
    }
    if !load_all(w, &mut keep, &spans) {
        return None;
    }
    Some(Ok(keep))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a plan to its end, loading from `file` what it asks for; the buffer's unloaded bytes are `fill`.
    pub(crate) fn run(file: &[u8], prefix: usize, fill: u8) -> (HeaderPlan, Vec<u8>, Ranges) {
        let mut buf = vec![fill; file.len()];
        let mut loaded = Ranges::new();
        let load = |buf: &mut Vec<u8>, loaded: &mut Ranges, a: u64, b: u64| {
            buf[a as usize..b as usize].copy_from_slice(&file[a as usize..b as usize]);
            loaded.insert(a, b);
        };
        load(&mut buf, &mut loaded, 0, prefix.min(file.len()) as u64);
        for _ in 0..32 {
            match header_plan(&buf, &loaded) {
                HeaderPlan::Need(r) => {
                    for &(a, b) in r.spans() {
                        assert!(!loaded.overlaps(a, b), "asked again for {a}..{b}");
                        load(&mut buf, &mut loaded, a, b);
                    }
                }
                done => return (done, buf, loaded),
            }
        }
        panic!("no answer after 32 rounds");
    }

    /// An 8 × 8 gray TIFF whose IFD0 carries `layers` bytes of private data (a Photoshop file's layer
    /// blob) and an Artist; returns the file and its strip.
    fn layered_tiff(layers: usize) -> (Vec<u8>, Vec<u8>) {
        use lightcraft_tiff::{IfdBuilder, ImageData, TiffWriter, Value};
        let strip: Vec<u8> = (0..64).collect();
        let mut ifd = IfdBuilder::new();
        ifd.set(t::IMAGE_WIDTH, Value::Long(vec![8]));
        ifd.set(t::IMAGE_LENGTH, Value::Long(vec![8]));
        ifd.set(t::BITS_PER_SAMPLE, Value::Short(vec![8]));
        ifd.set(t::COMPRESSION, Value::Short(vec![1]));
        ifd.set(t::PHOTOMETRIC, Value::Short(vec![1]));
        ifd.set(t::SAMPLES_PER_PIXEL, Value::Short(vec![1]));
        ifd.set(t::ARTIST, Value::Ascii("Someone with a long enough name".into()));
        ifd.set(37724, Value::Undefined(vec![7; layers]));
        ifd.set_image(ImageData::Strips { rows_per_strip: 8, strips: vec![strip.clone()] });
        (TiffWriter::default().write(&[ifd]).unwrap(), strip)
    }

    /// A TIFF with a huge private tag value (layers) is described and decoded from its tables, its small
    /// values and its first image's strips: the layer data is never asked for.
    #[test]
    fn image_plan_skips_layer_data() {
        let (file, strip) = layered_tiff(BIG_VALUE as usize + 1);
        let mut buf = vec![0u8; file.len()];
        let mut loaded = Ranges::new();
        loaded.insert(0, 16);
        buf[..16].copy_from_slice(&file[..16]);
        for _ in 0..8 {
            match image_plan(&buf, &loaded) {
                HeaderPlan::Need(r) => {
                    for &(a, b) in r.spans() {
                        assert!(!loaded.overlaps(a, b), "asked again for {a}..{b}");
                        buf[a as usize..b as usize].copy_from_slice(&file[a as usize..b as usize]);
                        loaded.insert(a, b);
                    }
                }
                HeaderPlan::Ready => {
                    assert!(loaded.total() < 4096, "read {} of {} bytes", loaded.total(), file.len());
                    let tiff = Tiff::parse(&buf).unwrap();
                    let ifd = &tiff.ifds[0];
                    assert_eq!(ifd.value(t::ARTIST).and_then(|v| v.as_str()).as_deref(), Some("Someone with a long enough name"));
                    let c = &ifd.image().unwrap().chunks(file.len() as u64)[0];
                    assert_eq!(&buf[c.offset as usize..(c.offset + c.len) as usize], &strip[..]);
                    return;
                }
                HeaderPlan::Whole(why) => panic!("whole file: {why}"),
            }
        }
        panic!("no answer");
    }

    #[test]
    fn image_plan_refuses_what_is_not_a_tiff() {
        let mut loaded = Ranges::new();
        loaded.insert(0, 32);
        assert!(matches!(image_plan(&[0u8; 32], &loaded), HeaderPlan::Whole(_)));
        assert!(matches!(image_plan(&[], &Ranges::new()), HeaderPlan::Whole(_)));
    }

    #[test]
    fn ranges_merge_and_subtract() {
        let mut r = Ranges::new();
        r.insert(10, 20);
        r.insert(30, 40);
        r.insert(20, 25); // touching: merged
        r.insert(5, 5); // empty
        assert_eq!(r.spans(), &[(10, 25), (30, 40)]);
        r.insert(0, 100);
        assert_eq!(r.spans(), &[(0, 100)]);
        let mut r = Ranges::new();
        r.insert(10, 20);
        r.insert(30, 40);
        assert!(r.contains(12, 18) && r.contains(5, 5) && !r.contains(15, 35));
        assert!(r.overlaps(15, 35) && !r.overlaps(20, 30));
        assert_eq!(r.gaps(0, 50).spans(), &[(0, 10), (20, 30), (40, 50)]);
        assert_eq!(r.gaps(12, 18).spans(), &[] as &[(u64, u64)]);
        let mut cut = Ranges::new();
        cut.insert(15, 32);
        assert_eq!(r.minus(&cut).spans(), &[(10, 15), (32, 40)]);
        assert_eq!(r.total(), 20);
    }

    #[test]
    fn hostile_input_never_panics_and_ends() {
        let mut loaded = Ranges::new();
        for len in [0usize, 1, 8, 15, 16, 200] {
            let bytes = vec![0u8; len];
            loaded.insert(0, len as u64);
            assert!(matches!(header_plan(&bytes, &loaded), HeaderPlan::Whole(_)));
        }
        // an IFD that points at itself, an IFD count past the end, a huge SubIFDs count
        let mut tiff = b"II*\0\x08\0\0\0".to_vec();
        tiff.extend_from_slice(&2u16.to_le_bytes());
        tiff.extend_from_slice(&[0x4a, 0x01, 4, 0, 0xff, 0xff, 0xff, 0x0f, 8, 0, 0, 0]); // SubIFDs, 0x0fffffff values at 8
        tiff.extend_from_slice(&[0x69, 0x87, 4, 0, 1, 0, 0, 0, 8, 0, 0, 0]); // Exif → itself
        tiff.extend_from_slice(&8u32.to_le_bytes()); // next → itself
        let (plan, _, _) = run(&tiff, 4, 0);
        assert!(matches!(plan, HeaderPlan::Whole(_)), "{plan:?}");
        // a CR3 whose `moov` claims more than the file holds: walked as the parser walks it (it
        // stops there), so the plan ends with the little there is, and the probe then fails as
        // it does on the whole file
        let mut cr3 = 24u32.to_be_bytes().to_vec();
        cr3.extend_from_slice(b"ftypcrx \0\0\0\x01crx isom");
        cr3.extend_from_slice(&u32::MAX.to_be_bytes());
        cr3.extend_from_slice(b"moov");
        cr3.resize(4096, 0);
        let (plan, buf, loaded) = run(&cr3, 16, 0);
        assert_eq!(plan, HeaderPlan::Ready);
        assert_eq!(loaded.total(), 24 + 32, "only the two box headers are read");
        assert_eq!(format!("{:?}", crate::probe_info(&buf)), format!("{:?}", crate::probe_info(&cr3)));
        // a RAF whose container lies outside the file
        let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        raf.resize(160, 0);
        raf[100..104].copy_from_slice(&1000u32.to_be_bytes());
        raf[104..108].copy_from_slice(&1000u32.to_be_bytes());
        assert_eq!(run(&raf, 160, 0).0, HeaderPlan::Whole("RAF raw container"));
    }

    /// An ARW6 strip's probe reads are not a prefix: the residual stream — whose first half fixes the first-half
    /// shift — starts behind other streams' data (the corpus's `arw6-sony-ilce7rm6-apsc-land-hq.arw` used to be
    /// described from zeros there and failed with "first-half row count outside 2..=7"). The plan keeps those
    /// bytes, wherever in the strip they sit.
    #[test]
    fn arw6_plan_keeps_the_scattered_probe_reads() {
        use crate::vendor::arw6_testenc::{Qis, arw6_file, one_tile_quantised};
        for file in [
            // one tile, like the ILCE-7RM6's APS-C crop
            arw6_file(&[(0, 0, 1024, 1024, one_tile_quantised(1024, 1024, 3, Qis::REAL).0)], 1024, 1024),
            // several tiles, like the full frame (each tile's reads are kept)
            arw6_file(
                &[
                    (0, 0, 1024, 1024, one_tile_quantised(1024, 1024, 0, Qis::REAL).0),
                    (1024, 0, 1024, 1024, one_tile_quantised(1024, 1024, 0, Qis::REAL).0),
                ],
                2048,
                1024,
            ),
        ] {
            for fill in [0u8, 0xff] {
                let (plan, buf, loaded) = run(&file, 512 * 1024, fill);
                assert!(matches!(plan, HeaderPlan::Ready), "{plan:?} (fill {fill})");
                // the plan asked for bytes past the prefix keep: the probe's reads off a prefix (zeros or 0xFF)
                // would break the first-half shift, the regression above
                assert!(loaded.spans().iter().any(|&(a, _)| a >= 512 * 1024), "nothing read past the prefix (fill {fill})");
                assert_eq!(format!("{:?}", crate::probe_info(&buf)), format!("{:?}", crate::probe_info(&file)), "fill {fill}");
            }
        }
    }

    /// The case a plan once declined (the whole file was read): a second tile past a first one of bulk size. The plan
    /// either describes the file as the whole file is described or reads it whole; it never describes it differently.
    #[test]
    fn arw6_second_tile_past_a_bulk_first_is_described_as_the_whole_file() {
        use crate::vendor::arw6_testenc::{Qis, arw6_file, one_tile_quantised};
        let (tile, _) = one_tile_quantised(64, 48, 0, Qis::REAL);
        let mut first = tile.clone();
        first.resize(BULK as usize, 0);
        let file = arw6_file(&[(0, 0, 64, 48, first), (64, 0, 64, 48, tile)], 128, 48);
        assert!(crate::probe_info(&file).is_ok());
        for fill in [0u8, 0xff] {
            match run(&file, 16, fill) {
                (HeaderPlan::Ready, buf, _) => {
                    assert_eq!(format!("{:?}", crate::probe_info(&buf)), format!("{:?}", crate::probe_info(&file)), "fill {fill}")
                }
                (HeaderPlan::Whole(_), ..) => {}
                (other, ..) => panic!("{other:?}"),
            }
        }
    }

    /// A TIFF with a million one-byte strips listed in descending order and all byte counts 0: the plan is
    /// built in one sort and merge, not one pass over all strips (or all holes) per strip.
    #[test]
    fn many_strips_without_byte_counts_plan_fast() {
        let n = 1_000_000u32;
        let (offsets_at, counts_at) = (8 + 2 + 5 * 12 + 4, 8 + 2 + 5 * 12 + 4 + 4 * n);
        let data = counts_at + 4 * n;
        let mut tiff = b"II*\0\x08\0\0\0".to_vec();
        tiff.extend_from_slice(&5u16.to_le_bytes());
        let entry = |tiff: &mut Vec<u8>, tag: u16, typ: u16, count: u32, value: u32| {
            tiff.extend_from_slice(&tag.to_le_bytes());
            tiff.extend_from_slice(&typ.to_le_bytes());
            tiff.extend_from_slice(&count.to_le_bytes());
            tiff.extend_from_slice(&value.to_le_bytes());
        };
        entry(&mut tiff, 256, 4, 1, 1);
        entry(&mut tiff, 257, 4, 1, n);
        entry(&mut tiff, 273, 4, n, offsets_at);
        entry(&mut tiff, 278, 4, 1, 1);
        entry(&mut tiff, 279, 4, n, counts_at);
        tiff.extend_from_slice(&0u32.to_le_bytes());
        for i in (0..n).rev() {
            tiff.extend_from_slice(&(data + i).to_le_bytes());
        }
        tiff.resize(data as usize + n as usize, 0);
        let len = tiff.len() as u64;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut loaded = Ranges::new();
            loaded.insert(0, len);
            let _ = tx.send(header_plan(&tiff, &loaded));
        });
        let plan = rx.recv_timeout(std::time::Duration::from_secs(20)).expect("header_plan took longer than 20 s");
        assert!(!matches!(plan, HeaderPlan::Need(_)), "{plan:?}");
    }
}
