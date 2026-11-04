//! Corrupt-file sweep: feed damaged copies of real sample files through everything an import reads, and report every
//! panic, every run over its time limit and every run that asks far more memory than the intact file needed.
//! `cargo xtask corrupt-sweep [--seed N] [--jobs N] [--fields N] [--mem-limit-gb N] [--time-factor F] [--min-secs S]
//! [DIR|FILE…]` (default: `$LIGHTCRAFT_CORPUS` or `corpus/`, and `apps/lightbites-swift/test-raws`). Run it before
//! every beta; it exits non-zero when it finds anything.
//!
//! **Mutants.** Per file, from a fixed seed (the same mutants on every machine and every run): the intact file (the
//! baseline), truncations at fixed offsets and fractions of the file, random byte flips (in the first 64 KiB, where the
//! headers are, and anywhere), and header fields set to all zeros and all ones: TIFF IFD entries (count, type, value or
//! offset, next IFD), ISO-BMFF box sizes and their first payload words (CR3, HEIC), JPEG segment lengths and frame
//! sizes (embedded previews too), and the first 256 bytes as 32-bit words (every other container's header).
//!
//! **Steps.** Each mutant goes through the engine's import probe (`files::probe_bytes`), the metadata reads (`meta`
//! extract and the Info panel's tag rows), the embedded preview / thumbnail (`raw::embedded_preview`, the import
//! thumbnail `files::embedded_preview_srgb`, the network-share range read), the full raw decode (`raw::probe_info` +
//! `raw::decode`) or the image codecs (header, full decode, thumbnail), the import preview render (`files::load_bytes`
//! at 512 px: the reduced and binned raw decoders), and for XMP sidecars the XMP parser and the sidecar reader.
//!
//! **Checks.** Each step runs under `catch_unwind`, and a panic hook records every panic, also one that a guard further
//! down catches. Each file runs in a child process (this binary with `--child`), one mutant at a time, so a stack
//! overflow or an abort is attributed to its mutant and the sweep goes on with the next; the parent kills a mutant
//! that runs longer than `--time-factor` times the intact file's time (at least `--min-secs`) and runs it again alone
//! with four times as long: a hang when that is not enough either (a loaded machine slows everything down). The
//! child's global allocator counts (`lightcraft_sysmem::CountingAlloc`): a mutant whose peak is over 4× the intact
//! file's peak plus 1 GiB, and over 32 bytes per pixel of the picture its header declares to the import probe, is
//! reported (a header may declare a bigger picture within the decoders' size caps: a JPEG of 4096 × 65535 needs
//! only 1.6 MB of image data), and a request past `--mem-limit-gb` is refused (the child aborts, reported as an
//! oversized allocation) instead of taking the machine's memory.
//!
//! **Reproducing.** Every finding prints the child command that runs that one mutant alone (add `--dump OUT` to write
//! the damaged file instead).
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOC: lightcraft_sysmem::CountingAlloc = lightcraft_sysmem::CountingAlloc;

const DEFAULT_SEED: u64 = 0x5eed_1c0f_2026_1104;
/// What a decode may hold per pixel of the picture it declares: 8-bit or 16-bit samples, the float working image
/// (12 bytes) and a resample or demosaic on the way.
const BYTES_PER_PIXEL: u64 = 32;
/// Header fields picked per file; each gives two mutants (zeros, ones).
const DEFAULT_FIELDS: usize = 16;
const EXTENSIONS: &[&str] = &[
    "3fr", "ari", "arw", "avif", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "gpr", "heic", "heif", "hif", "iiq", "jpeg", "jpg", "jxl", "kdc",
    "mef", "mos", "mrw", "nef", "nrw", "orf", "ori", "pef", "png", "psd", "raf", "rw2", "rwl", "sr2", "srf", "srw", "tif", "tiff", "webp", "x3f",
    "xmp",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = if args.first().map(String::as_str) == Some("--child") { child(&args[1..]) } else { parent(&args) };
    std::process::exit(code);
}

fn flag<T: std::str::FromStr>(args: &[String], name: &str) -> Option<T> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
}

// ---- mutants ----------------------------------------------------------------------------------------------------

/// splitmix64: a few lines, the same numbers everywhere.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

#[derive(Clone)]
struct Field {
    at: usize,
    width: usize,
    what: &'static str,
}

enum Mutation {
    Original,
    Truncate(usize),
    Flip(Vec<(usize, u8)>, &'static str),
    Set(Field, u8),
}

impl Mutation {
    fn apply(&self, b: &[u8]) -> Vec<u8> {
        match self {
            Mutation::Original => b.to_vec(),
            Mutation::Truncate(n) => b.get(..*n).unwrap_or(b).to_vec(),
            Mutation::Flip(flips, _) => {
                let mut v = b.to_vec();
                for &(at, x) in flips {
                    if let Some(c) = v.get_mut(at) {
                        *c ^= x;
                    }
                }
                v
            }
            Mutation::Set(f, byte) => {
                let mut v = b.to_vec();
                if let Some(s) = v.get_mut(f.at..f.at.saturating_add(f.width)) {
                    s.fill(*byte);
                }
                v
            }
        }
    }

    fn describe(&self) -> String {
        match self {
            Mutation::Original => "intact".into(),
            Mutation::Truncate(n) => format!("truncated to {n} bytes"),
            Mutation::Flip(flips, span) => format!("{} byte(s) flipped {span}", flips.len()),
            Mutation::Set(f, byte) => {
                format!("{} ({} byte(s) at {:#x}) set to {}", f.what, f.width, f.at, if *byte == 0 { "zeros" } else { "ones" })
            }
        }
    }
}

fn file_seed(seed: u64, name: &str) -> u64 {
    // FNV-1a of the file name (not its path: the same mutants in every checkout)
    name.bytes().fold(0xcbf2_9ce4_8422_2325_u64 ^ seed, |h, c| (h ^ u64::from(c)).wrapping_mul(0x100_0000_01b3))
}

fn mutants(b: &[u8], seed: u64, fields_wanted: usize) -> Vec<Mutation> {
    let n = b.len();
    let mut rng = Rng(seed);
    let mut out = vec![Mutation::Original];
    let mut cuts: Vec<usize> = vec![0, 1, 2, 4, 8, 12, 16, 32, 64, 512, 4096, 65536];
    cuts.extend([1, 5, 10, 25, 50, 75, 90, 99].map(|p| n / 100 * p));
    cuts.push(n.saturating_sub(1));
    cuts.retain(|&c| c < n);
    cuts.sort_unstable();
    cuts.dedup();
    out.extend(cuts.into_iter().map(Mutation::Truncate));
    for (k, count) in [1, 4, 16, 64, 1, 4, 16, 64].into_iter().enumerate() {
        let (span, label) = if k < 4 { (n.min(1 << 16), "in the first 64 KiB") } else { (n, "anywhere") };
        let flips = (0..count).map(|_| (rng.below(span), (rng.below(255) + 1) as u8)).collect();
        out.push(Mutation::Flip(flips, label));
    }
    let mut fields = fields(b);
    for i in (1..fields.len()).rev() {
        fields.swap(i, rng.below(i + 1));
    }
    for f in fields.into_iter().take(fields_wanted) {
        out.push(Mutation::Set(f.clone(), 0));
        out.push(Mutation::Set(f, 0xFF));
    }
    out
}

fn rd16(b: &[u8], at: usize, le: bool) -> Option<u16> {
    let s: [u8; 2] = b.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(if le { u16::from_le_bytes(s) } else { u16::from_be_bytes(s) })
}

fn rd32(b: &[u8], at: usize, le: bool) -> Option<u32> {
    let s: [u8; 4] = b.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(if le { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
}

fn rd64be(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

/// The header fields worth damaging: every container's first bytes, TIFF IFDs, ISO-BMFF boxes and JPEG segments.
fn fields(b: &[u8]) -> Vec<Field> {
    let mut f: Vec<Field> = (0..b.len().min(256)).step_by(4).map(|at| Field { at, width: 4, what: "header word" }).collect();
    tiff_fields(b, &mut f);
    if b.get(4..8) == Some(b"ftyp") {
        bmff_fields(b, 0, b.len(), 0, &mut f);
    }
    jpeg_fields(b, &mut f);
    f.retain(|x| x.at.checked_add(x.width).is_some_and(|e| e <= b.len()));
    f.sort_by_key(|x| (x.at, x.width));
    f.dedup_by_key(|x| (x.at, x.width));
    f
}

fn tiff_fields(b: &[u8], f: &mut Vec<Field>) {
    let mut headers = 0;
    let mut i = 0;
    while i + 8 <= b.len() && headers < 16 {
        let le = match b.get(i..i + 4) {
            Some(b"II*\0" | b"IIRO" | b"IIU\0" | b"IIRS") => true,
            Some(b"MM\0*" | b"MMOR") => false,
            _ => {
                i += 1;
                continue;
            }
        };
        if walk_ifds(b, i, le, f) {
            headers += 1;
        }
        i += 4;
    }
}

/// The IFDs of the TIFF at `base` (offsets relative to it), with their EXIF, GPS, interop and sub-IFDs. False when
/// nothing there looks like an IFD (a stray signature inside compressed data).
fn walk_ifds(b: &[u8], base: usize, le: bool, f: &mut Vec<Field>) -> bool {
    let Some(first) = rd32(b, base + 4, le) else { return false };
    let mut todo = vec![(first as usize, 0)];
    let mut seen = HashSet::new();
    let mut found = false;
    while let Some((off, depth)) = todo.pop() {
        if off == 0 || depth > 4 || seen.len() > 64 || !seen.insert(off) {
            continue;
        }
        let Some(at) = base.checked_add(off) else { continue };
        let Some(count) = rd16(b, at, le) else { continue };
        if count == 0 || count > 1000 || !rd16(b, at + 4, le).is_some_and(|t| (1..=13).contains(&t)) {
            continue;
        }
        found = true;
        f.push(Field { at, width: 2, what: "IFD entry count" });
        for e in 0..usize::from(count) {
            let ent = at + 2 + 12 * e;
            let Some(tag) = rd16(b, ent, le) else { break };
            f.push(Field { at: ent + 2, width: 2, what: "IFD entry type" });
            f.push(Field { at: ent + 4, width: 4, what: "IFD entry value count" });
            f.push(Field { at: ent + 8, width: 4, what: "IFD entry value or offset" });
            if matches!(tag, 0x8769 | 0x8825 | 0xA005 | 0x014A)
                && let Some(v) = rd32(b, ent + 8, le)
            {
                todo.push((v as usize, depth + 1));
            }
        }
        let next = at + 2 + 12 * usize::from(count);
        f.push(Field { at: next, width: 4, what: "next IFD offset" });
        if let Some(v) = rd32(b, next, le) {
            todo.push((v as usize, depth));
        }
    }
    found
}

fn bmff_fields(b: &[u8], start: usize, end: usize, depth: usize, f: &mut Vec<Field>) {
    let mut o = start;
    for _ in 0..256 {
        if depth > 6 || o.saturating_add(8) > end {
            return;
        }
        let (Some(size), Some(ty)) = (rd32(b, o, false), b.get(o + 4..o + 8)) else { return };
        if !ty.iter().all(|c| c.is_ascii_alphanumeric() || *c == b' ') {
            return;
        }
        let (header, len) = match size {
            1 => (16, rd64be(b, o + 8).and_then(|l| usize::try_from(l).ok()).unwrap_or(0)),
            0 => (8, end - o),
            s => (8, s as usize),
        };
        f.push(Field { at: o, width: 4, what: "box size" });
        if size == 1 {
            f.push(Field { at: o + 8, width: 8, what: "box large size" });
        }
        if len < header || o.saturating_add(len) > end {
            return;
        }
        let body = o + header;
        f.push(Field { at: body, width: 4, what: "box payload word (version, count, size)" });
        f.push(Field { at: body + 4, width: 4, what: "box payload word (version, count, size)" });
        let children = match ty {
            b"meta" => Some(body + 4),
            b"uuid" => Some(body + 16),
            b"stsd" => Some(body + 8),
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"dinf" | b"iprp" | b"ipco" | b"edts" => Some(body),
            _ => None,
        };
        if let Some(c) = children {
            bmff_fields(b, c, o + len, depth + 1, f);
        }
        o += len;
    }
}

fn jpeg_fields(b: &[u8], f: &mut Vec<Field>) {
    let mut from = 0;
    for _ in 0..8 {
        let Some(p) = b.get(from..).and_then(|s| s.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF])) else { return };
        let start = from + p;
        let mut o = start + 2;
        for _ in 0..64 {
            let (Some(&0xFF), Some(&m)) = (b.get(o), b.get(o + 1)) else { break };
            if m == 0xD9 || (0xD0..=0xD7).contains(&m) || m == 0xFF {
                break;
            }
            f.push(Field { at: o + 2, width: 2, what: "JPEG segment length" });
            if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
                f.push(Field { at: o + 5, width: 2, what: "JPEG frame height" });
                f.push(Field { at: o + 7, width: 2, what: "JPEG frame width" });
                f.push(Field { at: o + 9, width: 1, what: "JPEG frame component count" });
            }
            if m == 0xDA {
                f.push(Field { at: o + 4, width: 1, what: "JPEG scan component count" });
                break;
            }
            let Some(len) = rd16(b, o + 2, false) else { break };
            o += 2 + usize::from(len);
        }
        from = start + 3;
    }
}

// ---- child: one file, its mutants one at a time ---------------------------------------------------------------------

static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn one_line(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn child(args: &[String]) -> i32 {
    let Some(path) = args.first().map(PathBuf::from) else {
        eprintln!("--child FILE [--from I] [--only] [--dump OUT] [--seed N] [--fields N] [--mem-limit-gb N]");
        return 2;
    };
    let from: usize = flag(args, "--from").unwrap_or(0);
    let only = args.iter().any(|a| a == "--only");
    let seed = flag(args, "--seed").unwrap_or(DEFAULT_SEED);
    let fields_wanted = flag(args, "--fields").unwrap_or(DEFAULT_FIELDS);
    let mem_gb: usize = flag(args, "--mem-limit-gb").unwrap_or(6);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            return 2;
        }
    };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ms = mutants(&bytes, file_seed(seed, &name), fields_wanted);
    if let Some(out) = args.iter().position(|a| a == "--dump").and_then(|i| args.get(i + 1)) {
        let Some(m) = ms.get(from) else { return 2 };
        return match std::fs::write(out, m.apply(&bytes)) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{out}: {e}");
                2
            }
        };
    }
    std::panic::set_hook(Box::new(|info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        PANICS.lock().unwrap_or_else(PoisonError::into_inner).push(format!("{at}\t{}", one_line(&msg)));
    }));
    lightcraft_sysmem::set_counted_limit(mem_gb.saturating_mul(1 << 30));
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut out = std::io::stdout();
    let _ = writeln!(out, "count\t{}", ms.len());
    let end = if only { from.saturating_add(1).min(ms.len()) } else { ms.len() };
    for (i, m) in ms.iter().enumerate().take(end).skip(from) {
        let damaged = m.apply(&bytes);
        let _ = writeln!(out, "start\t{i}\t{}", m.describe());
        lightcraft_sysmem::reset_counted_peak();
        let (before, _) = lightcraft_sysmem::counted();
        let t = Instant::now();
        let declared = run_steps(&name, &ext, &damaged, i, &mut out);
        let (_, peak) = lightcraft_sysmem::counted();
        let _ = writeln!(out, "done\t{i}\t{}\t{}\t{declared}", t.elapsed().as_micros(), peak.saturating_sub(before));
    }
    0
}

fn step(out: &mut impl Write, i: usize, name: &str, f: impl FnOnce()) {
    let _ = writeln!(out, "step\t{i}\t{name}");
    PANICS.lock().unwrap_or_else(PoisonError::into_inner).clear();
    let t = Instant::now();
    let _ = catch_unwind(AssertUnwindSafe(f));
    let _ = writeln!(out, "took\t{i}\t{name}\t{}", t.elapsed().as_micros());
    for p in std::mem::take(&mut *PANICS.lock().unwrap_or_else(PoisonError::into_inner)) {
        let _ = writeln!(out, "panic\t{i}\t{name}\t{p}");
    }
}

/// Everything an import (and the first look at the photo) reads from the file. Returns the pixels the import probe
/// says the picture has (0 when it can't tell): what a decode may legitimately allocate for.
fn run_steps(name: &str, ext: &str, b: &[u8], i: usize, out: &mut impl Write) -> u64 {
    use lightcraft_engine::files;
    let mut declared = 0;
    step(out, i, "probe", || {
        if let Ok(p) = files::probe_bytes(name, b) {
            declared = u64::from(p.width) * u64::from(p.height);
        }
    });
    step(out, i, "meta", || {
        let _ = lightcraft_meta::try_extract(b);
        let _ = lightcraft_meta::file_tag_rows(b);
    });
    if ext == "xmp" {
        step(out, i, "xmp", || {
            let text = String::from_utf8_lossy(b);
            let _ = lightcraft_meta::parse_xmp(&text);
            let _ = lightcraft_engine::sidecar::parse_sidecar(&text, lightcraft_engine::crs::Target::Any);
        });
        return declared;
    }
    step(out, i, "preview", || {
        let _ = lightcraft_raw::embedded_preview(b);
        let _ = files::embedded_preview_srgb(b, 320);
        let prefix = b.get(..lightcraft_raw::PREVIEW_PREFIX).unwrap_or(b);
        if let Some(r) = lightcraft_raw::bounded_preview(prefix) {
            let start = usize::try_from(r.offset).unwrap_or(usize::MAX);
            let window = b.get(start..).unwrap_or_default();
            let _ = lightcraft_raw::embedded_preview_from_range(window, r);
        }
    });
    if lightcraft_raw::probe(b).is_some() {
        step(out, i, "raw", || {
            let _ = lightcraft_raw::probe_info(b);
            let _ = lightcraft_raw::decode(b);
        });
    } else {
        step(out, i, "codecs", || {
            let _ = lightcraft_codecs::read_header(b);
            let _ = lightcraft_codecs::decode(b, lightcraft_codecs::DecodeOptions::default());
            let _ = lightcraft_codecs::decode_thumbnail(b, 256);
        });
    }
    step(out, i, "load", || {
        let _ = files::load_bytes(b, 512);
    });
    declared
}

// ---- parent: files in parallel, a child process per file -----------------------------------------------------------

struct Opts {
    exe: PathBuf,
    child_args: Vec<String>,
    time_factor: f64,
    min_secs: f64,
}

struct Finding {
    kind: &'static str,
    step: String,
    key: String,
    detail: String,
    file: PathBuf,
    index: usize,
    mutation: String,
}

#[derive(Default)]
struct FileStat {
    mutants: usize,
    /// The intact file's time and peak memory.
    baseline: Option<(Duration, usize)>,
    /// Mutants over the time limit that finished when run alone: (index, mutation, time alone).
    slow: Vec<(usize, String, Duration)>,
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let Ok(rd) = std::fs::read_dir(path) else { return };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for e in entries {
            collect(&e, out);
        }
    } else if path.extension().is_some_and(|e| EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str())) {
        out.push(path.to_path_buf());
    }
}

fn parent(args: &[String]) -> i32 {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a.starts_with("--") {
            it.next();
        } else {
            paths.push(a.into());
        }
    }
    if paths.is_empty() {
        let corpus = std::env::var_os("LIGHTCRAFT_CORPUS").map_or_else(|| root.join("corpus"), PathBuf::from);
        paths = vec![corpus, root.join("apps/lightbites-swift/test-raws")];
    }
    let mut files = Vec::new();
    for p in &paths {
        if !p.exists() {
            eprintln!("note: {} is not there (corpus/ lives in the main checkout: set LIGHTCRAFT_CORPUS)", p.display());
        }
        collect(p, &mut files);
    }
    // biggest first: the long ones start early
    files.sort_by_key(|f| std::cmp::Reverse(std::fs::metadata(f).map(|m| m.len()).unwrap_or(0)));
    if files.is_empty() {
        eprintln!("no sample files found");
        return 2;
    }
    let jobs = flag(args, "--jobs").unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1)));
    let mut child_args = Vec::new();
    for name in ["--seed", "--fields", "--mem-limit-gb"] {
        if let Some(v) = flag::<String>(args, name) {
            child_args.extend([name.to_string(), v]);
        }
    }
    let opts = Arc::new(Opts {
        exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("corrupt_sweep")),
        child_args,
        time_factor: flag(args, "--time-factor").unwrap_or(10.0),
        min_secs: flag(args, "--min-secs").unwrap_or(20.0),
    });
    eprintln!("corrupt-file sweep: {} files, {jobs} at a time", files.len());
    let started = Instant::now();
    let queue = Arc::new(Mutex::new(files.into_iter().collect::<VecDeque<_>>()));
    let results = Arc::new(Mutex::new((Vec::<Finding>::new(), Vec::<(PathBuf, FileStat)>::new())));
    let workers: Vec<_> = (0..jobs)
        .map(|_| {
            let (queue, results, opts, root) = (Arc::clone(&queue), Arc::clone(&results), Arc::clone(&opts), root.clone());
            std::thread::spawn(move || {
                loop {
                    let Some(file) = queue.lock().unwrap_or_else(PoisonError::into_inner).pop_front() else { break };
                    let (findings, stat) = sweep_file(&opts, &file);
                    eprintln!("  {} mutants, {} finding(s): {}", stat.mutants, findings.len(), file.strip_prefix(&root).unwrap_or(&file).display());
                    let mut r = results.lock().unwrap_or_else(PoisonError::into_inner);
                    r.0.extend(findings);
                    r.1.push((file, stat));
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    let (findings, stats) = std::mem::take(&mut *results.lock().unwrap_or_else(PoisonError::into_inner));
    report(&findings, &stats, started.elapsed(), &opts)
}

/// Run every mutant of `file`, a child process at a time: a crash or a hang ends that child, and the next one goes on
/// after the mutant that caused it. A mutant over its time limit runs again alone with four times as long (a loaded
/// machine slows everything down): a hang only when it is over that too.
fn sweep_file(opts: &Opts, file: &Path) -> (Vec<Finding>, FileStat) {
    let mut findings = Vec::new();
    let mut stat = FileStat::default();
    let mut from = 0;
    let mut hangs = 0;
    while let Some(mut end) = run_child(opts, file, from, None, &mut stat, &mut findings) {
        if end.hung {
            match run_child(opts, file, end.index, Some(4.0), &mut stat, &mut findings) {
                Some(again) => end = again,
                None => {
                    from = end.index + 1;
                    continue;
                }
            }
        }
        let (kind, key) = if end.hung {
            hangs += 1;
            ("hang", format!("over {:.0} s", end.took.as_secs_f64()))
        } else if end.tail.contains("memory allocation of") {
            ("memory", "allocation refused (past --mem-limit-gb)".into())
        } else if end.tail.contains("overflowed its stack") {
            ("crash", "stack overflow".into())
        } else {
            ("crash", end.status.clone())
        };
        let index = end.index;
        findings.push(Finding { kind, step: end.step, key, detail: end.tail, file: file.to_path_buf(), index, mutation: end.mutation });
        if index == 0 || hangs >= 2 {
            eprintln!("{}: stopped after mutant {index} ({kind})", file.display());
            break;
        }
        from = index + 1;
        if stat.mutants > 0 && from >= stat.mutants {
            break;
        }
    }
    (findings, stat)
}

/// How a child stopped inside a mutant.
struct Ended {
    index: usize,
    mutation: String,
    step: String,
    took: Duration,
    hung: bool,
    tail: String,
    status: String,
}

/// One child running the mutants from `from` (`confirm`: only that one, with the time limit that many times longer;
/// its panics and peaks were reported by the run it repeats). `None` when it got through them all.
fn run_child(opts: &Opts, file: &Path, from: usize, confirm: Option<f64>, stat: &mut FileStat, findings: &mut Vec<Finding>) -> Option<Ended> {
    let mut cmd = Command::new(&opts.exe);
    cmd.arg("--child").arg(file).args(["--from", &from.to_string()]).args(&opts.child_args);
    if confirm.is_some() {
        cmd.arg("--only");
    }
    let spawned = cmd.env("RAYON_NUM_THREADS", "2").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: can't start the child: {e}", file.display());
            return None;
        }
    };
    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take();
    std::thread::spawn(move || {
        let Some(s) = stdout else { return };
        for line in BufReader::new(s).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let stderr = child.stderr.take();
    let err_reader = {
        let tail = Arc::clone(&tail);
        std::thread::spawn(move || {
            let Some(s) = stderr else { return };
            for line in BufReader::new(s).lines().map_while(Result::ok) {
                let mut t = tail.lock().unwrap_or_else(PoisonError::into_inner);
                t.push_back(line);
                if t.len() > 8 {
                    t.pop_front();
                }
            }
        })
    };
    // (index, description, started, step)
    let mut current: Option<(usize, String, Instant, String)> = None;
    let hung = loop {
        let limit = match stat.baseline {
            Some((t, _)) => (t.as_secs_f64() * opts.time_factor).max(opts.min_secs) * confirm.unwrap_or(1.0),
            None => 300.0,
        };
        let wait = current.as_ref().map_or(Duration::from_secs(600), |c| Duration::from_secs_f64(limit).saturating_sub(c.2.elapsed()));
        let line = match rx.recv_timeout(wait) {
            Ok(l) => l,
            Err(RecvTimeoutError::Timeout) => break true,
            Err(RecvTimeoutError::Disconnected) => break false,
        };
        let parts: Vec<&str> = line.split('\t').collect();
        let mutation = || current.as_ref().map(|c| c.1.clone()).unwrap_or_default();
        match parts.as_slice() {
            ["count", n] => stat.mutants = n.parse().unwrap_or(0),
            ["start", i, what] => current = Some((i.parse().unwrap_or(0), (*what).to_string(), Instant::now(), String::new())),
            ["step", _, s] => {
                if let Some(c) = current.as_mut() {
                    c.3 = (*s).to_string();
                }
            }
            ["panic", i, s, at, msg] if confirm.is_none() => findings.push(Finding {
                kind: "panic",
                step: (*s).to_string(),
                key: (*at).to_string(),
                detail: (*msg).to_string(),
                file: file.to_path_buf(),
                index: i.parse().unwrap_or(0),
                mutation: mutation(),
            }),
            ["done", i, micros, peak, declared] => {
                let (i, t, peak) = (i.parse().unwrap_or(0), Duration::from_micros(micros.parse().unwrap_or(0)), peak.parse::<usize>().unwrap_or(0));
                // A damaged header may declare a bigger picture, which the decoders' own size caps allow (a JPEG of
                // 4096 × 65535 needs no more than 1.6 MB of entropy-coded data): what is suspicious is a peak past
                // both the intact file's and what the declared picture needs.
                let declared_need = usize::try_from(declared.parse::<u64>().unwrap_or(0).saturating_mul(BYTES_PER_PIXEL)).unwrap_or(usize::MAX);
                match stat.baseline {
                    _ if confirm.is_some() => stat.slow.push((i, mutation(), t)),
                    None if i == 0 => stat.baseline = Some((t, peak)),
                    Some((_, base)) if peak > base.saturating_mul(4).saturating_add(1 << 30).max(declared_need) => findings.push(Finding {
                        kind: "memory",
                        step: "all".into(),
                        key: "peak far above the intact file's and the declared picture's".into(),
                        detail: format!("peak {} MB, intact file {} MB, declared picture {} MB", peak >> 20, base >> 20, declared_need >> 20),
                        file: file.to_path_buf(),
                        index: i,
                        mutation: mutation(),
                    }),
                    _ => {}
                }
                current = None;
            }
            _ => {}
        }
    };
    if hung {
        let _ = child.kill();
    }
    let status = child.wait().map(|s| s.to_string()).unwrap_or_else(|e| e.to_string());
    let _ = err_reader.join();
    let (index, mutation, began, step) = current?;
    let tail: Vec<String> = tail.lock().unwrap_or_else(PoisonError::into_inner).iter().cloned().collect();
    Some(Ended { index, mutation, step, took: began.elapsed(), hung, tail: one_line(&tail.join(" | ")), status })
}

fn report(findings: &[Finding], stats: &[(PathBuf, FileStat)], took: Duration, opts: &Opts) -> i32 {
    let mutants: usize = stats.iter().map(|(_, s)| s.mutants).sum();
    let mut groups: BTreeMap<(&str, &str, &str), Vec<&Finding>> = BTreeMap::new();
    for f in findings {
        groups.entry((f.kind, f.step.as_str(), f.key.as_str())).or_default().push(f);
    }
    println!("\n{} files, {mutants} mutants in {:.0} s: {} finding(s) in {} group(s)", stats.len(), took.as_secs_f64(), findings.len(), groups.len());
    for ((kind, step, key), list) in &groups {
        println!("\n[{kind}] step {step}: {key} ({} mutants)", list.len());
        for f in list.iter().take(3) {
            println!("    {} #{} ({}): {}", f.file.display(), f.index, f.mutation, f.detail);
            println!("      rerun: {} --child '{}' --from {} --only {}", opts.exe.display(), f.file.display(), f.index, opts.child_args.join(" "));
        }
    }
    let mut slow: Vec<(&PathBuf, &(usize, String, Duration))> = stats.iter().flat_map(|(p, s)| s.slow.iter().map(move |x| (p, x))).collect();
    slow.sort_by_key(|(_, x)| std::cmp::Reverse(x.2));
    if !slow.is_empty() {
        println!("\n{} mutant(s) passed the time limit but finished when run alone (a loaded machine; not counted):", slow.len());
        for (p, (i, what, t)) in slow.iter().take(5) {
            println!("    {:.1} s alone: {} #{i} ({what})", t.as_secs_f64(), p.display());
        }
    }
    let mut slow: Vec<&(PathBuf, FileStat)> = stats.iter().filter(|(_, s)| s.baseline.is_some()).collect();
    slow.sort_by_key(|(_, s)| std::cmp::Reverse(s.baseline.map(|b| b.0)));
    println!("\nslowest intact files:");
    for (p, s) in slow.iter().take(5) {
        if let Some((t, peak)) = s.baseline {
            println!("    {:.2} s, peak {} MB: {}", t.as_secs_f64(), peak >> 20, p.display());
        }
    }
    i32::from(!findings.is_empty())
}
