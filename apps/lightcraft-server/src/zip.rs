//! A ZIP archive of files on disk, streamed: **Download all** on an album link ([`crate::shares`]).
//!
//! The files are stored as they are (photos don't compress), so the archive's length is known before a byte
//! is read and the answer has a `Content-Length` (a browser shows how far the download is). Each file's CRC is
//! computed while it streams and written after it, in a data descriptor (general purpose bit 3), and again in
//! the central directory at the end. Files of 4 GiB and more, and archives that big, use the ZIP64 records
//! (APPNOTE 6.3.10 §4.3.9, §4.5.3), only where a field would overflow, so ordinary albums make ordinary ZIPs.
//!
//! A file that shrinks while it is sent is padded with zeros, and one that grows is cut, so the archive is
//! always as long as promised and its CRCs match what was sent.

use std::io::Read;
use std::path::PathBuf;

const LOCAL: u32 = 0x0403_4b50;
const DESCRIPTOR: u32 = 0x0807_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const END: u32 = 0x0605_4b50;
const END64: u32 = 0x0606_4b50;
const LOCATOR64: u32 = 0x0706_4b50;
/// Data descriptor (bit 3) and UTF-8 names (bit 11).
const FLAGS: u16 = 0x0808;
const MAX32: u64 = 0xFFFF_FFFF;

/// One file in the archive.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Its name in the archive (unique; no `/`).
    pub name: String,
    pub path: PathBuf,
    /// Its length when the archive was planned: what is sent.
    pub size: u64,
    /// MS-DOS time and date ([`dos_time`]).
    pub time: (u16, u16),
}

/// `YYYY-MM-DDTHH:MM:SS` → MS-DOS (time, date); 1980-01-01 for anything earlier or unreadable.
pub fn dos_time(iso: &str) -> (u16, u16) {
    let num = |a: usize, b: usize| iso.get(a..b).and_then(|s| s.parse::<u16>().ok());
    let (Some(y), Some(mo), Some(d)) = (num(0, 4), num(5, 7), num(8, 10)) else { return (0, 0x21) };
    if !(1980..=2107).contains(&y) || !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return (0, 0x21);
    }
    let (h, mi, s) = (num(11, 13).unwrap_or(0).min(23), num(14, 16).unwrap_or(0).min(59), num(17, 19).unwrap_or(0).min(59));
    ((h << 11) | (mi << 5) | (s / 2), ((y - 1980) << 9) | (mo << 5) | d)
}

/// Where each part of the archive goes, worked out before anything is sent.
struct Plan {
    /// Each entry's local header offset.
    offsets: Vec<u64>,
    central_at: u64,
    central_len: u64,
    len: u64,
}

fn big(size: u64, limit: u64) -> bool {
    size >= limit
}

fn local_len(e: &Entry, limit: u64) -> u64 {
    30 + e.name.len() as u64 + if big(e.size, limit) { 20 } else { 0 }
}

fn descriptor_len(e: &Entry, limit: u64) -> u64 {
    if big(e.size, limit) { 24 } else { 16 }
}

/// The ZIP64 extra field of a central directory record: the sizes and the offset that don't fit.
fn central_extra(e: &Entry, offset: u64, limit: u64) -> Vec<u8> {
    let mut v = Vec::new();
    if big(e.size, limit) {
        v.extend(e.size.to_le_bytes());
        v.extend(e.size.to_le_bytes());
    }
    if offset >= limit {
        v.extend(offset.to_le_bytes());
    }
    if v.is_empty() {
        return v;
    }
    let mut x = Vec::with_capacity(v.len() + 4);
    x.extend(1u16.to_le_bytes());
    x.extend((v.len() as u16).to_le_bytes());
    x.extend(v);
    x
}

fn central_len(e: &Entry, offset: u64, limit: u64) -> u64 {
    46 + e.name.len() as u64 + central_extra(e, offset, limit).len() as u64
}

fn plan(entries: &[Entry], limit: u64) -> Plan {
    let mut at = 0u64;
    let mut offsets = Vec::with_capacity(entries.len());
    for e in entries {
        offsets.push(at);
        at = at.saturating_add(local_len(e, limit)).saturating_add(e.size).saturating_add(descriptor_len(e, limit));
    }
    let central_at = at;
    let central_len: u64 = entries.iter().zip(&offsets).map(|(e, o)| central_len(e, *o, limit)).fold(0, u64::saturating_add);
    let end = if needs_end64(entries.len(), central_at, central_len, limit) { 56 + 20 + 22 } else { 22 };
    Plan { offsets, central_at, central_len, len: central_at.saturating_add(central_len).saturating_add(end) }
}

fn needs_end64(count: usize, central_at: u64, central_len: u64, limit: u64) -> bool {
    count >= 0xFFFF || central_at >= limit || central_len >= limit
}

/// What is being sent.
enum Step {
    /// Entry `i`'s local header, then its data.
    Local(usize),
    /// Entry `i`'s data: the file (if it opened) and the bytes still to send.
    Data(usize, Option<std::fs::File>, u64),
    Descriptor(usize),
    Central,
    Done,
}

/// The archive, as a reader. [`Zip::len`] is exactly what it reads.
pub struct Zip {
    entries: Vec<Entry>,
    plan: Plan,
    /// Sizes from here up are written as ZIP64 (`0xFFFF_FFFF`; tests make it small).
    limit: u64,
    crcs: Vec<u32>,
    crc: flate2::Crc,
    step: Step,
    /// Bytes made and not read yet.
    pending: Vec<u8>,
    at: usize,
}

impl Zip {
    /// An archive of `entries` (their names must be unique).
    pub fn new(entries: Vec<Entry>) -> Zip {
        Zip::with_limit(entries, MAX32)
    }

    fn with_limit(entries: Vec<Entry>, limit: u64) -> Zip {
        let plan = plan(&entries, limit);
        let crcs = vec![0; entries.len()];
        let step = if entries.is_empty() { Step::Central } else { Step::Local(0) };
        Zip { entries, plan, limit, crcs, crc: flate2::Crc::new(), step, pending: Vec::new(), at: 0 }
    }

    /// Bytes in the archive.
    pub fn len(&self) -> u64 {
        self.plan.len
    }

    pub fn is_empty(&self) -> bool {
        self.plan.len == 0
    }

    fn local(&self, i: usize) -> Vec<u8> {
        let Some(e) = self.entries.get(i) else { return Vec::new() };
        let big = big(e.size, self.limit);
        let mut v = Vec::with_capacity(local_len(e, self.limit) as usize);
        v.extend(LOCAL.to_le_bytes());
        v.extend(if big { 45u16 } else { 20u16 }.to_le_bytes());
        v.extend(FLAGS.to_le_bytes());
        v.extend(0u16.to_le_bytes()); // stored
        v.extend(e.time.0.to_le_bytes());
        v.extend(e.time.1.to_le_bytes());
        v.extend(0u32.to_le_bytes()); // CRC: in the descriptor
        let size = if big { u32::MAX } else { 0 };
        v.extend(size.to_le_bytes());
        v.extend(size.to_le_bytes());
        v.extend((e.name.len() as u16).to_le_bytes());
        v.extend(if big { 20u16 } else { 0u16 }.to_le_bytes());
        v.extend(e.name.as_bytes());
        if big {
            v.extend(1u16.to_le_bytes());
            v.extend(16u16.to_le_bytes());
            v.extend(0u64.to_le_bytes());
            v.extend(0u64.to_le_bytes());
        }
        v
    }

    fn descriptor(&self, i: usize) -> Vec<u8> {
        let Some(e) = self.entries.get(i) else { return Vec::new() };
        let mut v = Vec::with_capacity(24);
        v.extend(DESCRIPTOR.to_le_bytes());
        v.extend(self.crcs.get(i).copied().unwrap_or(0).to_le_bytes());
        if big(e.size, self.limit) {
            v.extend(e.size.to_le_bytes());
            v.extend(e.size.to_le_bytes());
        } else {
            v.extend((e.size as u32).to_le_bytes());
            v.extend((e.size as u32).to_le_bytes());
        }
        v
    }

    /// The central directory and the end records.
    fn central(&self) -> Vec<u8> {
        let limit = self.limit;
        let mut v = Vec::with_capacity(self.plan.central_len as usize + 98);
        for (i, e) in self.entries.iter().enumerate() {
            let offset = self.plan.offsets.get(i).copied().unwrap_or(0);
            let big = big(e.size, limit);
            let extra = central_extra(e, offset, limit);
            let version: u16 = if extra.is_empty() { 20 } else { 45 };
            v.extend(CENTRAL.to_le_bytes());
            v.extend(version.to_le_bytes()); // made by: MS-DOS
            v.extend(version.to_le_bytes());
            v.extend(FLAGS.to_le_bytes());
            v.extend(0u16.to_le_bytes());
            v.extend(e.time.0.to_le_bytes());
            v.extend(e.time.1.to_le_bytes());
            v.extend(self.crcs.get(i).copied().unwrap_or(0).to_le_bytes());
            let size = if big { u32::MAX } else { e.size as u32 };
            v.extend(size.to_le_bytes());
            v.extend(size.to_le_bytes());
            v.extend((e.name.len() as u16).to_le_bytes());
            v.extend((extra.len() as u16).to_le_bytes());
            v.extend(0u16.to_le_bytes()); // comment
            v.extend(0u16.to_le_bytes()); // disk
            v.extend(0u16.to_le_bytes()); // internal attributes
            v.extend(0u32.to_le_bytes()); // external attributes
            v.extend(if offset >= limit { u32::MAX } else { offset as u32 }.to_le_bytes());
            v.extend(e.name.as_bytes());
            v.extend(extra);
        }
        let (count, at, len) = (self.entries.len(), self.plan.central_at, self.plan.central_len);
        let wide = needs_end64(count, at, len, limit);
        if wide {
            let end64_at = at.saturating_add(len);
            v.extend(END64.to_le_bytes());
            v.extend(44u64.to_le_bytes());
            v.extend(45u16.to_le_bytes());
            v.extend(45u16.to_le_bytes());
            v.extend(0u32.to_le_bytes());
            v.extend(0u32.to_le_bytes());
            v.extend((count as u64).to_le_bytes());
            v.extend((count as u64).to_le_bytes());
            v.extend(len.to_le_bytes());
            v.extend(at.to_le_bytes());
            v.extend(LOCATOR64.to_le_bytes());
            v.extend(0u32.to_le_bytes());
            v.extend(end64_at.to_le_bytes());
            v.extend(1u32.to_le_bytes());
        }
        let n16 = if wide && count >= 0xFFFF { u16::MAX } else { count as u16 };
        v.extend(END.to_le_bytes());
        v.extend(0u16.to_le_bytes());
        v.extend(0u16.to_le_bytes());
        v.extend(n16.to_le_bytes());
        v.extend(n16.to_le_bytes());
        v.extend(if len >= limit { u32::MAX } else { len as u32 }.to_le_bytes());
        v.extend(if at >= limit { u32::MAX } else { at as u32 }.to_le_bytes());
        v.extend(0u16.to_le_bytes());
        v
    }

    fn next_entry(&self, i: usize) -> Step {
        if i + 1 < self.entries.len() { Step::Local(i + 1) } else { Step::Central }
    }
}

impl Read for Zip {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(rest) = self.pending.get(self.at..)
                && !rest.is_empty()
            {
                let n = rest.len().min(out.len());
                if let (Some(dst), Some(src)) = (out.get_mut(..n), rest.get(..n)) {
                    dst.copy_from_slice(src);
                }
                self.at += n;
                return Ok(n);
            }
            self.pending.clear();
            self.at = 0;
            match std::mem::replace(&mut self.step, Step::Done) {
                Step::Local(i) => {
                    self.pending = self.local(i);
                    let e = self.entries.get(i);
                    let file = e.and_then(|e| std::fs::File::open(&e.path).ok());
                    self.crc.reset();
                    self.step = Step::Data(i, file, e.map_or(0, |e| e.size));
                }
                Step::Data(i, file, 0) => {
                    drop(file);
                    if let Some(c) = self.crcs.get_mut(i) {
                        *c = self.crc.sum();
                    }
                    self.step = Step::Descriptor(i);
                }
                Step::Data(i, mut file, left) => {
                    let want = usize::try_from(left).unwrap_or(usize::MAX).min(out.len());
                    let Some(dst) = out.get_mut(..want) else { return Ok(0) };
                    let n = match file.as_mut().map(|f| f.read(dst)) {
                        Some(Ok(n)) if n > 0 => n,
                        Some(Err(e)) if e.kind() == std::io::ErrorKind::Interrupted => {
                            self.step = Step::Data(i, file, left);
                            continue;
                        }
                        // (shorter than planned, or unreadable: zeros, so the archive stays as long as promised)
                        _ => {
                            dst.fill(0);
                            file = None;
                            want
                        }
                    };
                    let n = n.min(want);
                    self.crc.update(out.get(..n).unwrap_or(&[]));
                    self.step = Step::Data(i, file, left - n as u64);
                    return Ok(n);
                }
                Step::Descriptor(i) => {
                    self.pending = self.descriptor(i);
                    self.step = self.next_entry(i);
                }
                Step::Central => {
                    self.pending = self.central();
                    self.step = Step::Done;
                }
                Step::Done => return Ok(0),
            }
        }
    }
}

/// Unique names for the files of an archive: `IMG_1.JPG`, `IMG_1 (2).JPG`… (case-insensitively, as on the
/// disks they are unpacked to).
pub fn unique_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    names
        .into_iter()
        .map(|n| {
            let (stem, ext) = match n.rfind('.') {
                Some(i) if i > 0 => (n.get(..i).unwrap_or(n), n.get(i..).unwrap_or("")),
                _ => (n, ""),
            };
            let mut name = n.to_string();
            let mut k = 2;
            while !seen.insert(name.to_lowercase()) {
                name = format!("{stem} ({k}){ext}");
                k += 1;
            }
            name
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(b: &[u8], i: usize) -> u16 {
        u16::from_le_bytes([b[i], b[i + 1]])
    }
    fn u32_at(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
    }
    fn u64_at(b: &[u8], i: usize) -> u64 {
        u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
    }

    /// Read an archive back by its central directory: (name, data) of each file, checked against its CRC
    /// and its local header.
    fn unzip(z: &[u8]) -> Vec<(String, Vec<u8>)> {
        let end = z.len() - 22;
        assert_eq!(u32_at(z, end), END);
        let (mut count, mut size, mut at) = (u64::from(u16_at(z, end + 10)), u64::from(u32_at(z, end + 12)), u64::from(u32_at(z, end + 16)));
        if u32_at(z, end) == END && (at == u64::from(u32::MAX) || count == 0xFFFF) {
            let loc = end - 20;
            assert_eq!(u32_at(z, loc), LOCATOR64);
            let e64 = u64_at(z, loc + 8) as usize;
            assert_eq!(u32_at(z, e64), END64);
            (count, size, at) = (u64_at(z, e64 + 32), u64_at(z, e64 + 40), u64_at(z, e64 + 48));
        }
        let mut out = Vec::new();
        let mut p = at as usize;
        for _ in 0..count {
            assert_eq!(u32_at(z, p), CENTRAL);
            let crc = u32_at(z, p + 16);
            let (mut csize, mut usize_) = (u64::from(u32_at(z, p + 20)), u64::from(u32_at(z, p + 24)));
            let (nlen, xlen) = (u16_at(z, p + 28) as usize, u16_at(z, p + 30) as usize);
            let mut offset = u64::from(u32_at(z, p + 42));
            let name = String::from_utf8(z[p + 46..p + 46 + nlen].to_vec()).unwrap();
            let mut x = p + 46 + nlen;
            if xlen > 0 {
                assert_eq!(u16_at(z, x), 1, "the ZIP64 extra");
                x += 4;
                if usize_ == u64::from(u32::MAX) {
                    usize_ = u64_at(z, x);
                    csize = u64_at(z, x + 8);
                    x += 16;
                }
                if offset == u64::from(u32::MAX) {
                    offset = u64_at(z, x);
                }
            }
            assert_eq!(csize, usize_);
            let l = offset as usize;
            assert_eq!(u32_at(z, l), LOCAL);
            assert_eq!(u16_at(z, l + 6), FLAGS);
            let data_at = l + 30 + u16_at(z, l + 26) as usize + u16_at(z, l + 28) as usize;
            let data = z[data_at..data_at + usize_ as usize].to_vec();
            let mut c = flate2::Crc::new();
            c.update(&data);
            assert_eq!(c.sum(), crc, "{name}");
            assert_eq!(u32_at(z, data_at + data.len()), DESCRIPTOR);
            assert_eq!(u32_at(z, data_at + data.len() + 4), crc);
            out.push((name, data));
            p += 46 + nlen + xlen;
        }
        assert_eq!(p as u64, at + size);
        out
    }

    fn files(dir: &std::path::Path, list: &[(&str, &[u8])]) -> Vec<Entry> {
        std::fs::create_dir_all(dir).unwrap();
        list.iter()
            .map(|(name, data)| {
                let path = dir.join(name);
                std::fs::write(&path, data).unwrap();
                Entry { name: name.to_string(), path, size: data.len() as u64, time: dos_time("2026-03-12T14:03:22") }
            })
            .collect()
    }

    fn read_all(mut z: Zip, chunk: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = vec![0u8; chunk];
        loop {
            let n = z.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        out
    }

    #[test]
    fn an_archive_is_as_long_as_planned_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("lc-zip-{}", std::process::id()));
        let big: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let list: [(&str, &[u8]); 3] = [("a.jpg", b"hello"), ("empty.png", b""), ("b.dng", &big)];
        for chunk in [1, 7, 65_536] {
            let z = Zip::new(files(&dir, &list));
            let len = z.len();
            let bytes = read_all(z, chunk);
            assert_eq!(bytes.len() as u64, len, "chunk {chunk}");
            let back = unzip(&bytes);
            assert_eq!(back.len(), 3);
            for ((name, data), (n, d)) in back.iter().zip(list) {
                assert_eq!((name.as_str(), data.as_slice()), (n, d));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zip64_records_where_a_field_would_overflow() {
        let dir = std::env::temp_dir().join(format!("lc-zip64-{}", std::process::id()));
        let list: [(&str, &[u8]); 3] = [("small", b"abc"), ("over", &[9u8; 150]), ("late", b"xyz")];
        // (a limit of 100 bytes stands in for 4 GiB)
        let z = Zip::with_limit(files(&dir, &list), 100);
        let len = z.len();
        let bytes = read_all(z, 4096);
        assert_eq!(bytes.len() as u64, len);
        let back = unzip(&bytes);
        assert_eq!(back.iter().map(|(n, d)| (n.as_str(), d.len())).collect::<Vec<_>>(), [("small", 3), ("over", 150), ("late", 3)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_changed_size_keeps_the_archive_whole() {
        let dir = std::env::temp_dir().join(format!("lc-zip-shrink-{}", std::process::id()));
        let mut entries = files(&dir, &[("a", b"0123456789"), ("gone", b"abc")]);
        entries[0].size = 20; // it shrank since it was planned
        entries[1].path = dir.join("no such file");
        let z = Zip::new(entries);
        let len = z.len();
        let bytes = read_all(z, 3);
        assert_eq!(bytes.len() as u64, len);
        let back = unzip(&bytes);
        assert_eq!(back[0].1, b"0123456789\0\0\0\0\0\0\0\0\0\0");
        assert_eq!(back[1].1, b"\0\0\0");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_archives_and_names() {
        let z = Zip::new(Vec::new());
        assert_eq!(z.len(), 22);
        assert_eq!(read_all(z, 100).len(), 22);
        assert_eq!(unique_names(["a.jpg", "A.JPG", "a.jpg", "b", "b", ".x"]), ["a.jpg", "A (2).JPG", "a (3).jpg", "b", "b (2)", ".x"]);
        assert_eq!(dos_time("2026-03-12T14:03:22"), ((14 << 11) | (3 << 5) | 11, (46 << 9) | (3 << 5) | 12));
        assert_eq!(dos_time("1970-01-01T00:00:00"), (0, 0x21));
        assert_eq!(dos_time("garbage"), (0, 0x21));
    }
}
