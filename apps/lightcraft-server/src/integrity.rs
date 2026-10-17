//! Checking a user's originals for files cut short: a copy or download that stopped part-way keeps the
//! start of the photo and loses its end. Such a file still opens (a raw shows its embedded camera JPEG,
//! a DNG or CR2 develops from what is left), so nothing else notices: the photo just looks wrong.
//!
//! Each file is checked from its structure, reading only its headers: every top-level box of an ISO
//! media file (CR3, HEIC, MP4, MOV) must end inside the file, every strip and tile a TIFF-based file
//! (DNG, CR2, NEF, ARW, ORF, RW2, PEF, TIFF) points at must lie inside it, a JPEG, PNG or GIF must end with
//! its end marker, and a WebP or PSD must be as long as it says. Formats it doesn't know are counted, not
//! judged. Nothing is changed: the admin page (Folders → Check files) and `lightcraft-server check` show
//! what is damaged, and the report stays in the user's folder (`integrity.json`) until the next check.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

/// The report's file in the user's folder.
const REPORT: &str = "integrity.json";
/// The most bytes one read takes (a directory of offsets, a file's last bytes).
const READ_MAX: u64 = 1 << 20;
/// Files read at once: the disks are the limit, and a NAS slows down with more.
const THREADS: usize = 4;

/// What a file's structure says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// Cut short: `missing` bytes (when the file says how long it should be) are gone from its end.
    Cut { missing: Option<u64>, why: String },
    /// A format this check doesn't read.
    Unchecked,
}

/// One file of a user's library that is damaged or couldn't be read.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Damaged {
    /// In a library folder: its place there (`Photos/2024/a.CR3`); kept by the server: `uploads/<hash>`.
    pub path: String,
    pub size: u64,
    /// Bytes missing from its end, when the file says how long it should be.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<u64>,
    /// What is wrong, in words.
    pub why: String,
    /// The library's photos of this file (their file names).
    pub photos: Vec<String>,
}

/// The last check of a user's originals.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Report {
    /// When it finished (seconds since 1970).
    pub finished: u64,
    /// Files checked.
    pub files: usize,
    /// Files whose format this check doesn't read.
    pub unchecked: usize,
    pub cut: Vec<Damaged>,
    /// Files that couldn't be opened or read.
    pub unreadable: Vec<Damaged>,
}

impl Report {
    pub fn load(user_dir: &Path) -> Option<Report> {
        serde_json::from_slice(&std::fs::read(user_dir.join(REPORT)).ok()?).ok()
    }

    pub fn save(&self, user_dir: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        let path = user_dir.join(REPORT);
        lightcraft_engine::catalog::safe_file::write_atomic(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// A file to check: where it is, what to call it, and the photos it is.
#[derive(Clone, Debug)]
pub struct Original {
    pub file: PathBuf,
    pub path: String,
    pub photos: Vec<String>,
}

/// Each content hash's photos (their file names) in a library.
pub fn photo_names(catalog: &lightcraft_catalog::Catalog) -> HashMap<String, Vec<String>> {
    let mut names: HashMap<String, Vec<String>> = HashMap::new();
    for p in catalog.photos() {
        if let Some(h) = &p.content_hash {
            names.entry(h.clone()).or_default().push(p.file_name.clone());
        }
    }
    names
}

/// The originals in a user's library folders (`index`, as last scanned).
pub fn folder_originals(index: &crate::folders::Index, names: &HashMap<String, Vec<String>>) -> Vec<Original> {
    let photos = |hash: &str| names.get(hash).cloned().unwrap_or_default();
    // (files the scan couldn't read too: a damaged one is often why)
    let found = index.files.iter().filter(|(_, e)| !e.missing);
    found.filter_map(|(key, e)| Some(Original { file: index.path_of(key)?, path: key.clone(), photos: photos(&e.hash) })).collect()
}

/// The originals the server keeps for a user (`<user dir>/blobs/original`, devices' uploads).
pub fn kept_originals(user_dir: &Path, names: &HashMap<String, Vec<String>>) -> Vec<Original> {
    let mut out = Vec::new();
    let kept = user_dir.join("blobs").join("original");
    for shard in std::fs::read_dir(&kept).into_iter().flatten().flatten() {
        if shard.file_name().len() != 2 {
            continue;
        }
        for f in std::fs::read_dir(shard.path()).into_iter().flatten().flatten() {
            let hash = f.file_name().to_string_lossy().to_string();
            if lightcraft_preview::Hash128::parse(&hash).is_none() {
                continue;
            }
            out.push(Original { file: f.path(), path: format!("uploads/{hash}"), photos: names.get(&hash).cloned().unwrap_or_default() });
        }
    }
    out
}

/// Check `files`, `THREADS` at a time, counting each in `done`; stops early when `stop` is set.
pub fn check_all(files: &[Original], done: &AtomicUsize, stop: &AtomicBool) -> Report {
    let next = AtomicUsize::new(0);
    let report = Mutex::new(Report::default());
    std::thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let Some(o) = files.get(next.fetch_add(1, Ordering::Relaxed)) else { break };
                    let size = std::fs::metadata(&o.file).map(|m| m.len()).unwrap_or(0);
                    let found = check_file(&o.file);
                    let mut r = report.lock().unwrap_or_else(PoisonError::into_inner);
                    r.files += 1;
                    let entry = |missing, why| Damaged { path: o.path.clone(), size, missing, why, photos: o.photos.clone() };
                    match found {
                        Ok(Verdict::Ok) => {}
                        Ok(Verdict::Unchecked) => r.unchecked += 1,
                        Ok(Verdict::Cut { missing, why }) => r.cut.push(entry(missing, why)),
                        Err(e) => r.unreadable.push(entry(None, e)),
                    }
                    drop(r);
                    done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    let mut r = report.into_inner().unwrap_or_else(PoisonError::into_inner);
    r.cut.sort_by(|a, b| a.path.cmp(&b.path));
    r.unreadable.sort_by(|a, b| a.path.cmp(&b.path));
    r.finished = crate::accounts::now();
    r
}

/// Check one file. `Err`: it couldn't be opened or read.
pub fn check_file(path: &Path) -> Result<Verdict, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    check(&mut f, len).map_err(|e| e.to_string())
}

/// Check a file of `len` bytes.
pub fn check<R: Read + Seek>(r: &mut R, len: u64) -> std::io::Result<Verdict> {
    if len == 0 {
        return Ok(Verdict::Cut { missing: None, why: "the file is empty".into() });
    }
    let mut f = Reader { r, len };
    let head = f.at(0, 16)?;
    let tiff_magic: [&[u8]; 5] = [b"II*\0", b"MM\0*", b"IIRO", b"IIRS", b"IIU\0"];
    if head.get(4..8) == Some(b"ftyp".as_slice()) {
        bmff(&mut f)
    } else if tiff_magic.iter().any(|m| head.starts_with(m)) {
        tiff(&mut f)
    } else if head.starts_with(&[0xff, 0xd8]) {
        let tail = f.tail(4096)?;
        // (a phone JPEG may end in zeros after, or instead of, its end marker: such files decode whole)
        // ponytail: a download that stopped in a run of zeros passes too; decoding the whole file would tell
        let ended = tail.ends_with(&[0xff, 0xd9]) || tail.iter().rev().take_while(|b| **b == 0).count() >= 2;
        Ok(if ended { Verdict::Ok } else { cut(None, "the JPEG has no end marker: its end is missing") })
    } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        let tail = f.tail(64)?;
        Ok(if tail.windows(4).any(|w| w == b"IEND") { Verdict::Ok } else { cut(None, "the PNG has no end chunk: its end is missing") })
    } else if head.starts_with(b"RIFF") {
        let need = u64::from(u32le(&head, 4)?).saturating_add(8);
        Ok(if need <= len { Verdict::Ok } else { cut(Some(need - len), "the file is shorter than it says") })
    } else if head.starts_with(b"GIF8") {
        Ok(if f.tail(1)? == b";" { Verdict::Ok } else { cut(None, "the GIF has no trailer: its end is missing") })
    } else if head.starts_with(b"8BPS") {
        psd(&mut f, head.get(4..6) == Some([0, 2].as_slice()))
    } else {
        Ok(Verdict::Unchecked)
    }
}

fn cut(missing: Option<u64>, why: &str) -> Verdict {
    Verdict::Cut { missing, why: why.into() }
}

struct Reader<'a, R> {
    r: &'a mut R,
    len: u64,
}

impl<R: Read + Seek> Reader<'_, R> {
    /// Up to `n` bytes at `off` (fewer at the end of the file).
    fn at(&mut self, off: u64, n: u64) -> std::io::Result<Vec<u8>> {
        let n = n.min(READ_MAX).min(self.len.saturating_sub(off));
        let mut b = vec![0; usize::try_from(n).unwrap_or(0)];
        if !b.is_empty() {
            self.r.seek(SeekFrom::Start(off))?;
            self.r.read_exact(&mut b)?;
        }
        Ok(b)
    }

    fn tail(&mut self, n: u64) -> std::io::Result<Vec<u8>> {
        self.at(self.len.saturating_sub(n), n)
    }
}

fn bad(why: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, why.to_string())
}

fn u32le(b: &[u8], at: usize) -> std::io::Result<u32> {
    b.get(at..at + 4).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes).ok_or_else(|| bad("short read"))
}

/// ISO media (CR3, HEIC, MP4, MOV): each top-level box must end inside the file.
fn bmff<R: Read + Seek>(f: &mut Reader<R>) -> std::io::Result<Verdict> {
    let mut off = 0u64;
    for _ in 0..100_000 {
        if off >= f.len {
            return Ok(Verdict::Ok);
        }
        let h = f.at(off, 16)?;
        let (Some(size), Some(kind)) = (h.get(0..4), h.get(4..8)) else {
            return Ok(cut(None, "the file ends inside a box header"));
        };
        let kind = String::from_utf8_lossy(kind).to_string();
        let mut size = u64::from(u32::from_be_bytes([size[0], size[1], size[2], size[3]]));
        if size == 1 {
            let Some(large) = h.get(8..16) else { return Ok(cut(None, "the file ends inside a box header")) };
            size = u64::from_be_bytes(large.try_into().map_err(|_| bad("short read"))?);
        } else if size == 0 {
            // (the last box, to the end of the file)
            return Ok(Verdict::Ok);
        }
        if size < 8 {
            return Ok(Verdict::Unchecked);
        }
        let end = off.saturating_add(size);
        if end > f.len {
            return Ok(cut(Some(end - f.len), &format!("the '{kind}' box ends past the end of the file")));
        }
        off = end;
    }
    Ok(Verdict::Unchecked)
}

/// TIFF-based files: every strip, tile and JPEG the directories point at (IFD chain and SubIFDs) must lie
/// inside the file.
fn tiff<R: Read + Seek>(f: &mut Reader<R>) -> std::io::Result<Verdict> {
    let h = f.at(0, 8)?;
    let le = h.starts_with(b"II");
    let u16_ = |b: &[u8], at: usize| -> Option<u64> {
        let s: [u8; 2] = b.get(at..at + 2)?.try_into().ok()?;
        Some(u64::from(if le { u16::from_le_bytes(s) } else { u16::from_be_bytes(s) }))
    };
    let u32_ = |b: &[u8], at: usize| -> Option<u64> {
        let s: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
        Some(u64::from(if le { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) }))
    };
    let first = u32_(&h, 4).ok_or_else(|| bad("short TIFF header"))?;
    let (mut far, mut seen, mut todo) = (0u64, std::collections::HashSet::new(), vec![(first, 0u32)]);
    while let Some((mut ifd, depth)) = todo.pop() {
        while ifd != 0 && depth < 8 && seen.len() < 200 && seen.insert(ifd) {
            let Some(n) = u16_(&f.at(ifd, 2)?, 0) else {
                return Ok(cut(None, "a directory lies past the end of the file"));
            };
            let size = n * 12 + 4;
            let ent = f.at(ifd + 2, size)?;
            if (ent.len() as u64) < size {
                return Ok(cut(None, "a directory runs past the end of the file"));
            }
            let mut tags: HashMap<u64, (u64, u64, &[u8])> = HashMap::new();
            for e in ent.chunks_exact(12).take(usize::try_from(n).unwrap_or(0)) {
                if let (Some(tag), Some(typ), Some(count)) = (u16_(e, 0), u16_(e, 2), u32_(e, 4)) {
                    tags.insert(tag, (typ, count, e.get(8..12).unwrap_or(&[])));
                }
            }
            // a tag's numbers (offsets and lengths are SHORT or LONG; anything else is skipped)
            let mut values = |tag: u64| -> std::io::Result<Option<Vec<u64>>> {
                let Some(&(typ, count, raw)) = tags.get(&tag) else { return Ok(None) };
                let width = match typ {
                    3 | 8 => 2,
                    4 | 9 | 13 => 4,
                    _ => return Ok(None),
                };
                let bytes = count.saturating_mul(width);
                if bytes > READ_MAX {
                    return Ok(None);
                }
                let data = if bytes <= 4 { raw.to_vec() } else { f.at(u32_(raw, 0).unwrap_or(u64::MAX), bytes)? };
                if (data.len() as u64) < bytes {
                    return Err(bad("values past the end"));
                }
                let read = |i: usize| if width == 2 { u16_(&data, i * 2) } else { u32_(&data, i * 4) };
                Ok(Some((0..usize::try_from(count).unwrap_or(0)).filter_map(read).collect()))
            };
            for (offsets, lengths) in [(273, 279), (324, 325), (513, 514)] {
                match (values(offsets), values(lengths)) {
                    (Ok(Some(o)), Ok(Some(l))) => far = o.iter().zip(&l).map(|(o, l)| o.saturating_add(*l)).fold(far, u64::max),
                    (Err(_), _) | (_, Err(_)) => return Ok(cut(None, "a directory's values lie past the end of the file")),
                    _ => {}
                }
            }
            match values(330) {
                Ok(Some(subs)) => todo.extend(subs.into_iter().map(|s| (s, depth + 1))),
                Ok(None) => {}
                Err(_) => return Ok(cut(None, "a directory's values lie past the end of the file")),
            }
            ifd = u32_(&ent, usize::try_from(n * 12).unwrap_or(usize::MAX)).unwrap_or(0);
        }
    }
    Ok(if far > f.len { cut(Some(far - f.len), "image data runs past the end of the file") } else { Verdict::Ok })
}

/// Photoshop (`large`: PSB): the colour mode, image resources and layers sections must fit in the file.
fn psd<R: Read + Seek>(f: &mut Reader<R>, large: bool) -> std::io::Result<Verdict> {
    let mut off = 26u64;
    for section in 0..3 {
        let width = if section == 2 && large { 8 } else { 4 };
        let b = f.at(off, width)?;
        if (b.len() as u64) < width {
            return Ok(cut(None, "the file ends inside a section header"));
        }
        let n = b.iter().fold(0u64, |a, x| a.saturating_mul(256).saturating_add(u64::from(*x)));
        off = off.saturating_add(width).saturating_add(n);
    }
    Ok(if off > f.len { cut(Some(off - f.len), "a section runs past the end of the file") } else { Verdict::Ok })
}

/// A check running or done, per user: how far it is.
#[derive(Default)]
struct Job {
    running: AtomicBool,
    done: AtomicUsize,
    total: AtomicUsize,
}

/// The admin page's checks, one per user at a time.
#[derive(Default)]
pub struct Checker {
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    stop: Arc<AtomicBool>,
}

impl Checker {
    /// Start checking `user`'s `files` (the report goes into `user_dir`). `false`: one is running already.
    pub fn start(&self, user: &str, user_dir: PathBuf, files: Vec<Original>) -> bool {
        let job = self.jobs.lock().unwrap_or_else(PoisonError::into_inner).entry(user.to_string()).or_default().clone();
        if job.running.swap(true, Ordering::AcqRel) {
            return false;
        }
        job.done.store(0, Ordering::Relaxed);
        job.total.store(files.len(), Ordering::Relaxed);
        let (stop, name, j) = (self.stop.clone(), user.to_string(), job.clone());
        let spawned = std::thread::Builder::new().name("lc-check".into()).spawn(move || {
            let report = check_all(&files, &j.done, &stop);
            if !stop.load(Ordering::Relaxed) {
                match report.save(&user_dir) {
                    Ok(()) => log::info!("{name}: checked {} files: {} cut short, {} unreadable", report.files, report.cut.len(), report.unreadable.len()),
                    Err(e) => log::error!("{name}: the check's report wasn't saved: {e}"),
                }
            }
            j.running.store(false, Ordering::Release);
        });
        if spawned.is_err() {
            job.running.store(false, Ordering::Release);
            return false;
        }
        true
    }

    /// `{running, done, total, report}` for the admin page.
    pub fn status(&self, user: &str, user_dir: &Path) -> serde_json::Value {
        let job = self.jobs.lock().unwrap_or_else(PoisonError::into_inner).get(user).cloned();
        let (running, done, total) = job.map_or((false, 0, 0), |j| (j.running.load(Ordering::Acquire), j.done.load(Ordering::Relaxed), j.total.load(Ordering::Relaxed)));
        serde_json::json!({"running": running, "done": done, "total": total, "report": Report::load(user_dir)})
    }

    /// Stop the running checks (the server is stopping): what they found so far isn't saved.
    pub(crate) fn shut_down(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn run(b: &[u8]) -> Verdict {
        check(&mut Cursor::new(b), b.len() as u64).unwrap()
    }

    /// A CR3-like file: ftyp, moov and an mdat holding the picture.
    fn bmff_file(mdat: usize) -> Vec<u8> {
        let mut b = Vec::new();
        for (kind, body) in [(b"ftyp", 12usize), (b"moov", 40), (b"mdat", mdat)] {
            b.extend(((8 + body) as u32).to_be_bytes());
            b.extend(kind);
            b.extend(std::iter::repeat_n(7u8, body));
        }
        b
    }

    /// A little-endian TIFF: IFD0 with one strip, and a SubIFD with another (as a CR2 / NEF / DNG keeps its raw).
    fn tiff_file() -> Vec<u8> {
        let mut b = b"II*\0".to_vec();
        b.extend(8u32.to_le_bytes());
        let entry = |tag: u16, typ: u16, count: u32, value: u32| [tag.to_le_bytes().as_slice(), &typ.to_le_bytes(), &count.to_le_bytes(), &value.to_le_bytes()].concat();
        // IFD0 at 8: strip at 100 (50 bytes), SubIFD at 60
        b.extend(3u16.to_le_bytes());
        b.extend(entry(273, 4, 1, 100));
        b.extend(entry(279, 4, 1, 50));
        b.extend(entry(330, 4, 1, 60));
        b.extend(0u32.to_le_bytes());
        b.resize(60, 0);
        // SubIFD at 60: strip at 150 (1000 bytes)
        b.extend(2u16.to_le_bytes());
        b.extend(entry(273, 4, 1, 150));
        b.extend(entry(279, 4, 1, 1000));
        b.extend(0u32.to_le_bytes());
        b.resize(1150, 9);
        b
    }

    #[test]
    fn whole_files_pass_and_cut_ones_say_how_much_is_missing() {
        let cr3 = bmff_file(5000);
        assert_eq!(run(&cr3), Verdict::Ok);
        let short = &cr3[..cr3.len() - 1234];
        assert!(matches!(run(short), Verdict::Cut { missing: Some(1234), ref why } if why.contains("mdat")), "{:?}", run(short));
        let tif = tiff_file();
        assert_eq!(run(&tif), Verdict::Ok);
        // the SubIFD's strip (the raw) is what a cut copy loses first
        assert!(matches!(run(&tif[..1100]), Verdict::Cut { missing: Some(50), .. }), "{:?}", run(&tif[..1100]));
        // cut before a directory: no length to compare with, still cut
        assert!(matches!(run(&tif[..62]), Verdict::Cut { missing: None, .. }));
    }

    #[test]
    fn end_markers_and_declared_lengths() {
        let jpeg = [&[0xff, 0xd8, 0xff, 0xe0][..], &[1; 100], &[0xff, 0xd9]].concat();
        assert_eq!(run(&jpeg), Verdict::Ok);
        assert!(matches!(run(&jpeg[..80]), Verdict::Cut { missing: None, .. }));
        // zero padding after the end marker, or in its place (phone JPEGs that decode whole)
        assert_eq!(run(&[jpeg.as_slice(), &[0; 300]].concat()), Verdict::Ok);
        let png = [&b"\x89PNG\r\n\x1a\n"[..], &[0; 40], b"\0\0\0\0IEND\xae\x42\x60\x82"].concat();
        assert_eq!(run(&png), Verdict::Ok);
        assert!(matches!(run(&png[..30]), Verdict::Cut { .. }));
        let mut webp = b"RIFF".to_vec();
        webp.extend(92u32.to_le_bytes());
        webp.extend(b"WEBP");
        webp.resize(100, 0);
        assert_eq!(run(&webp), Verdict::Ok);
        assert_eq!(run(&webp[..90]), Verdict::Cut { missing: Some(10), why: "the file is shorter than it says".into() });
        assert_eq!(run(b"something else entirely"), Verdict::Unchecked);
        assert!(matches!(run(b""), Verdict::Cut { .. }));
    }

    /// Hostile headers (lengths and offsets pointing anywhere, loops) give an answer, never a panic or a hang.
    #[test]
    fn hostile_headers_never_panic() {
        let mut loopy = tiff_file();
        // the SubIFD points back at IFD0
        loopy[8 + 2 + 24 + 8..8 + 2 + 24 + 12].copy_from_slice(&8u32.to_le_bytes());
        let _ = run(&loopy);
        for n in [0usize, 1, 3, 7, 9, 15, 17, 40] {
            for b in [bmff_file(10), tiff_file(), b"8BPS\0\x01".to_vec()] {
                let _ = check(&mut Cursor::new(&b[..n.min(b.len())]), n.min(b.len()) as u64);
            }
        }
        let mut huge = b"II*\0".to_vec();
        huge.extend(u32::MAX.to_le_bytes());
        assert!(matches!(run(&huge), Verdict::Cut { .. }));
        let big_box = [&u32::to_be_bytes(1)[..], b"ftyp", &u64::MAX.to_be_bytes()].concat();
        assert!(matches!(run(&big_box), Verdict::Cut { .. }));
    }
}
