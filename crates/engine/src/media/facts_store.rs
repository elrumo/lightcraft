//! What a decode of a raw found that a frontend drawing the photo itself asks for (its camera tone curve, the scene level
//! and colour of its picture, its lens data, and whether it carries a camera look, a local tone map or mattes), kept on
//! disk in the library's [`FOLDER`] so that each raw is decoded for them once per build of the app rather than once per
//! session.
//!
//! [`crate::Session::ensure_decoded`] decodes a raw at thumbnail size the first time in a session that such a frontend asks
//! about it (about 0.1 s on the engine thread, more than 1 s for a big CR3), though the answer depends on the file alone.
//! A record is valid for one file (its path, size, modification time and content hash), one imported camera profile, one
//! hot pixel setting and one build of the app (the executable's size and modification time). Change any of those and the
//! photo is decoded again and its record replaced. A camera look, a local tone map and mattes are too big to keep here: a
//! record says only whether the decode found one, and asking for it decodes the photo as before.
//!
//! **Layout.** 256 shard files `decode-facts/xx.lcf`, photos grouped [`GROUP`] at a time, so consecutive photos share a
//! shard (paging through a shoot reads one or two). A shard is a run of records `[u32 length][u64 FNV-1a of the
//! payload][payload]`, appended; a photo's last record wins. A record that is cut short or doesn't match its checksum ends
//! the shard: the shard is rewritten without it and what followed, and so is a shard holding more dead bytes than live
//! ones. Nothing here is the user's: deleting the folder only costs decodes.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::EmbeddedLens;
use lightcraft_pipeline::tone::{CHROMA_N, CameraTone};
use lightcraft_preview::{Lru, WriteGate};

/// The library's folder of kept decode facts.
pub const FOLDER: &str = "decode-facts";
/// Raise when what a record holds, or how a decode computes it, changes in a way the build identity can't see.
const FORMAT: u64 = 1;
const SHARDS: u64 = 256;
/// Photos (by id) that share a shard before the next shard takes over.
const GROUP: u64 = 64;
/// No record is bigger (a few KB in practice: the lens data is the variable part).
const MAX_RECORD: usize = 64 << 10;
/// A shard bigger than this is not read (and is rewritten with what this session stores in it).
const MAX_SHARD: u64 = 64 << 20;
/// Bytes of records read this session kept in memory.
const MEMO_BYTES: usize = 8 << 20;
const HEADER: usize = 12;
/// The most colour cells a record holds ([`crate::scene_level::colour_grid`] has 96).
const MAX_CELLS: usize = 4096;

const TONE: u8 = 1;
const LEVEL: u8 = 2;
const COLOUR: u8 = 4;
const LENS: u8 = 8;
const MATCH: u8 = 16;
const LOCAL_TONE: u8 = 32;
const MATTES: u8 = 64;

/// What a decode of a raw found, as a frontend that develops raws itself asks for it.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredFacts {
    /// The record's validity key ([`key`]).
    pub key: u64,
    /// The lens data the decoder found ([`lightcraft_pipeline::SourceInfo::lens`]).
    pub lens: Option<EmbeddedLens>,
    pub camera_tone: Option<CameraTone>,
    /// [`crate::Session::scene_level_of`] and [`crate::Session::scene_colour_of`] of the decoded picture.
    pub level: Option<[f32; 9]>,
    pub colour: Option<Vec<Option<[f32; 3]>>>,
    /// Whether the decode found a camera look, a local tone map and mattes (not kept: asking for one decodes).
    pub camera_match: bool,
    pub local_tone: bool,
    pub mattes: bool,
}

/// What a caller of [`crate::Session::ensure_decoded`] needs answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// The lens data, camera tone curve and scene level and colour.
    Basic,
    CameraMatch,
    LocalTone,
    Mattes,
}

impl StoredFacts {
    /// Whether these facts answer `need` without a decode: everything but a camera look, local tone map or mattes the
    /// decode found.
    pub fn covers(&self, need: Need) -> bool {
        match need {
            Need::Basic => true,
            Need::CameraMatch => !self.camera_match,
            Need::LocalTone => !self.local_tone,
            Need::Mattes => !self.mattes,
        }
    }

    fn bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.colour.as_ref().map_or(0, |c| c.len() * std::mem::size_of::<Option<[f32; 3]>>())
    }
}

/// The validity key of a raw's facts: its file at `path` as it is now (size, modification time, and the catalog's
/// `content_hash` of it), the imported camera `profile` it is developed with, whether hot pixels are removed, and this
/// build of the app. `None` when the file can't be looked at (offline, gone) or the build can't be told: then nothing is
/// kept or read.
pub fn key(path: &str, content_hash: Option<&str>, profile: Option<&str>, outliers: bool) -> Option<u64> {
    let build = build_identity()?;
    let meta = std::fs::metadata(path).ok().filter(std::fs::Metadata::is_file)?;
    let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(key_of(build, path, meta.len(), modified, content_hash, profile, outliers))
}

fn key_of(
    build: u64,
    path: &str,
    size: u64,
    modified: std::time::Duration,
    content_hash: Option<&str>,
    profile: Option<&str>,
    outliers: bool,
) -> u64 {
    let mut h = Fnv::new();
    h.u64(FORMAT);
    h.u64(build);
    h.text(path);
    h.u64(size);
    h.u64(modified.as_secs());
    h.u64(u64::from(modified.subsec_nanos()));
    h.text(content_hash.unwrap_or(""));
    h.text(profile.unwrap_or(""));
    h.u64(u64::from(outliers));
    h.0
}

/// This build of the app: its version and its executable's size and modification time. Asked once.
fn build_identity() -> Option<u64> {
    static ID: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        let meta = std::fs::metadata(exe).ok()?;
        let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
        let mut h = Fnv::new();
        h.text(env!("CARGO_PKG_VERSION"));
        h.u64(meta.len());
        h.u64(modified.as_secs());
        h.u64(u64::from(modified.subsec_nanos()));
        Some(h.0)
    })
}

/// One shard's records: where each photo's last one is.
#[derive(Default)]
struct Shard {
    index: HashMap<u64, Entry>,
    /// Where the next record goes: the end of the records read.
    end: u64,
}

#[derive(Clone, Copy)]
struct Entry {
    key: u64,
    at: u64,
    len: usize,
}

/// The kept decode facts of the open library ([`FOLDER`]); detached (keeping and finding nothing) for a library that is
/// not on disk.
pub struct FactsStore {
    dir: Option<PathBuf>,
    gate: Option<WriteGate>,
    shards: HashMap<u64, Shard>,
    memo: Lru<PhotoId, Arc<StoredFacts>>,
}

impl Default for FactsStore {
    fn default() -> Self {
        FactsStore { dir: None, gate: None, shards: HashMap::new(), memo: Lru::new(MEMO_BYTES) }
    }
}

impl FactsStore {
    /// Keep facts in `dir` (created when the first is kept), writing only while `gate` allows.
    pub fn attach(&mut self, dir: PathBuf, gate: Option<WriteGate>) {
        *self = FactsStore { dir: Some(dir), gate, ..FactsStore::default() };
    }

    pub fn detach(&mut self) {
        *self = FactsStore::default();
    }

    /// Forget what was read of photo `id` this session (its file changed).
    pub fn forget(&mut self, id: PhotoId) {
        self.memo.remove(&id);
    }

    /// Forget what was read this session (photo ids changed meaning). The records stay: each is checked against its file.
    pub fn clear_memo(&mut self) {
        self.memo = Lru::new(MEMO_BYTES);
        self.shards.clear();
    }

    /// The facts of photo `id` read or kept this session.
    pub fn peek(&self, id: PhotoId) -> Option<&Arc<StoredFacts>> {
        self.memo.peek(&id)
    }

    /// Photo `id`'s kept facts if they are for `key`.
    pub fn get(&mut self, id: PhotoId, key: u64) -> Option<Arc<StoredFacts>> {
        if let Some(f) = self.memo.get(&id).filter(|f| f.key == key) {
            return Some(f.clone());
        }
        let dir = self.dir.clone()?;
        let entry = self.shard(&dir, id)?.index.get(&id.0).copied().filter(|e| e.key == key)?;
        let facts = Arc::new(read_record(&shard_path(&dir, id), entry).filter(|f| f.key == key)?);
        self.memo.insert(id, facts.clone(), facts.bytes());
        Some(facts)
    }

    /// Keep `facts` for photo `id` (a record already kept for the same key stays). Failing to write costs only a decode
    /// next session.
    pub fn put(&mut self, id: PhotoId, facts: StoredFacts) {
        let key = facts.key;
        let facts = Arc::new(facts);
        self.memo.insert(id, facts.clone(), facts.bytes());
        let Some(dir) = self.dir.clone() else { return };
        if self.gate.as_ref().is_some_and(|allowed| !allowed()) {
            return;
        }
        let Some(record) = encode(id, &facts) else { return };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = shard_path(&dir, id);
        let Some(shard) = self.shard(&dir, id) else { return };
        if shard.index.get(&id.0).is_some_and(|e| e.key == key) {
            return;
        }
        match append(&path, shard.end, &record) {
            Ok(()) => {
                shard.index.insert(id.0, Entry { key, at: shard.end, len: record.len() });
                shard.end = shard.end.saturating_add(record.len() as u64);
            }
            Err(e) => log::debug!("{}: {e}", path.display()),
        }
    }

    /// Photo `id`'s shard, read the first time it is asked for (and rewritten then when it needs it).
    fn shard(&mut self, dir: &Path, id: PhotoId) -> Option<&mut Shard> {
        let n = shard_of(id);
        if !self.shards.contains_key(&n) {
            let writable = self.gate.as_ref().is_none_or(|allowed| allowed());
            let shard = load_shard(&shard_path(dir, id), writable)?;
            self.shards.insert(n, shard);
        }
        self.shards.get_mut(&n)
    }
}

fn shard_of(id: PhotoId) -> u64 {
    (id.0 / GROUP) % SHARDS
}

fn shard_path(dir: &Path, id: PhotoId) -> PathBuf {
    dir.join(format!("{:02x}.lcf", shard_of(id)))
}

/// Read a shard's index. A missing shard is an empty one; one that can't be read is `None` (nothing kept in it this
/// session). A damaged end, or more dead records than live ones, is rewritten away when `writable`.
fn load_shard(path: &Path, writable: bool) -> Option<Shard> {
    let size = match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Shard::default()),
        Err(_) => return None,
        Ok(m) => m.len(),
    };
    let bytes = if size > MAX_SHARD { Vec::new() } else { std::fs::read(path).ok()? };
    let (shard, valid) = index(&bytes);
    let live: usize = shard.index.values().map(|e| e.len).sum();
    let damaged = size > MAX_SHARD || valid < bytes.len();
    if writable && (damaged || (valid > 16 << 10 && valid - live > live)) {
        return Some(rewrite(path, &bytes, shard));
    }
    Some(shard)
}

/// `shard` with only its live records, written over `path` (`bytes` is what was read of it). Unchanged if the write fails.
fn rewrite(path: &Path, bytes: &[u8], shard: Shard) -> Shard {
    let mut entries: Vec<(u64, Entry)> = shard.index.iter().map(|(id, e)| (*id, *e)).collect();
    entries.sort_by_key(|(_, e)| e.at);
    let mut out = Vec::new();
    let mut index = HashMap::with_capacity(entries.len());
    for (id, e) in entries {
        let Some(rec) = usize::try_from(e.at).ok().and_then(|at| bytes.get(at..at.checked_add(e.len)?)) else { continue };
        index.insert(id, Entry { at: out.len() as u64, ..e });
        out.extend_from_slice(rec);
    }
    match lightcraft_catalog::safe_file::write_atomic_nosync(path, &out) {
        Ok(()) => Shard { index, end: out.len() as u64 },
        Err(e) => {
            log::debug!("{}: {e}", path.display());
            shard
        }
    }
}

/// The last record of each photo in `bytes`, and how far the records that check out reach.
fn index(bytes: &[u8]) -> (Shard, usize) {
    let mut shard = Shard::default();
    let mut at = 0usize;
    while let Some((id, key, len)) = frame(bytes, at) {
        shard.index.insert(id, Entry { key, at: at as u64, len });
        at += len;
    }
    shard.end = at as u64;
    (shard, at)
}

/// The record at `at`: its photo id, key and whole length, if it is all there and matches its checksum.
fn frame(bytes: &[u8], at: usize) -> Option<(u64, u64, usize)> {
    let head = bytes.get(at..at.checked_add(HEADER)?)?;
    let len = u32::from_le_bytes(head.get(..4)?.try_into().ok()?) as usize;
    let sum = u64::from_le_bytes(head.get(4..12)?.try_into().ok()?);
    if len > MAX_RECORD {
        return None;
    }
    let start = at.checked_add(HEADER)?;
    let payload = bytes.get(start..start.checked_add(len)?)?;
    if fnv(payload) != sum {
        return None;
    }
    let mut r = Reader { bytes: payload, at: 0 };
    Some((r.u64()?, r.u64()?, HEADER + len))
}

fn read_record(path: &Path, e: Entry) -> Option<StoredFacts> {
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(e.at)).ok()?;
    let mut bytes = vec![0; e.len];
    f.read_exact(&mut bytes).ok()?;
    frame(&bytes, 0)?;
    decode(bytes.get(HEADER..)?)
}

/// Write `record` at `end`, cutting off anything after it first (the cut-short tail of a write that didn't finish).
fn append(path: &Path, end: u64, record: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path)?;
    if f.metadata()?.len() != end {
        f.set_len(end)?;
    }
    f.seek(SeekFrom::Start(end))?;
    f.write_all(record)
}

/// The whole record of `facts`: header and payload. `None` if it would be too big.
fn encode(id: PhotoId, facts: &StoredFacts) -> Option<Vec<u8>> {
    let mut p = Vec::with_capacity(1024);
    p.extend_from_slice(&id.0.to_le_bytes());
    p.extend_from_slice(&facts.key.to_le_bytes());
    let lens = facts.lens.as_ref().and_then(|l| serde_json::to_vec(l).ok());
    let flags = [
        (facts.camera_tone.is_some(), TONE),
        (facts.level.is_some(), LEVEL),
        (facts.colour.is_some(), COLOUR),
        (lens.is_some(), LENS),
        (facts.camera_match, MATCH),
        (facts.local_tone, LOCAL_TONE),
        (facts.mattes, MATTES),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .fold(0u8, |a, (_, bit)| a | bit);
    p.push(flags);
    if let Some(t) = &facts.camera_tone {
        floats(&mut p, t.knots().as_flattened());
        floats(&mut p, t.chroma());
    }
    if let Some(l) = &facts.level {
        floats(&mut p, l);
    }
    if let Some(cells) = &facts.colour {
        write_cells(&mut p, cells)?;
    }
    if let Some(l) = &lens {
        p.extend_from_slice(&(l.len() as u32).to_le_bytes());
        p.extend_from_slice(l);
    }
    if p.len() > MAX_RECORD {
        return None;
    }
    let mut out = Vec::with_capacity(HEADER + p.len());
    out.extend_from_slice(&(p.len() as u32).to_le_bytes());
    out.extend_from_slice(&fnv(&p).to_le_bytes());
    out.extend_from_slice(&p);
    Some(out)
}

fn floats(p: &mut Vec<u8>, v: &[f32]) {
    v.iter().for_each(|x| p.extend_from_slice(&x.to_le_bytes()));
}

/// A colour grid: its length, which cells are there (a bit each), then their colours. `None` past [`MAX_CELLS`].
fn write_cells(p: &mut Vec<u8>, cells: &[Option<[f32; 3]>]) -> Option<()> {
    let n = u16::try_from(cells.len()).ok().filter(|n| usize::from(*n) <= MAX_CELLS)?;
    p.extend_from_slice(&n.to_le_bytes());
    let mut seen = vec![0u8; cells.len().div_ceil(8)];
    for (i, c) in cells.iter().enumerate() {
        if c.is_some()
            && let Some(b) = seen.get_mut(i / 8)
        {
            *b |= 1 << (i % 8);
        }
    }
    p.extend_from_slice(&seen);
    for c in cells.iter().flatten() {
        floats(p, c);
    }
    Some(())
}

/// A record's payload read back; `None` for one that doesn't make sense (it is then decoded again).
fn decode(payload: &[u8]) -> Option<StoredFacts> {
    let mut r = Reader { bytes: payload, at: 0 };
    let (_id, key, flags) = (r.u64()?, r.u64()?, r.u8()?);
    let on = |bit: u8| flags & bit != 0;
    let camera_tone = if on(TONE) { Some(read_tone(&mut r)?) } else { None };
    let level = if on(LEVEL) { Some(r.finite::<9>()?) } else { None };
    let colour = if on(COLOUR) { Some(read_cells(&mut r)?) } else { None };
    let lens = if on(LENS) {
        let n = r.u32()? as usize;
        Some(serde_json::from_slice::<EmbeddedLens>(r.take(n)?).ok()?)
    } else {
        None
    };
    (r.at == payload.len()).then_some(StoredFacts {
        key,
        lens,
        camera_tone,
        level,
        colour,
        camera_match: on(MATCH),
        local_tone: on(LOCAL_TONE),
        mattes: on(MATTES),
    })
}

/// A camera tone curve, checked as one read from a file is ([`CameraTone::new`], [`CameraTone::with_chroma`]).
fn read_tone(r: &mut Reader<'_>) -> Option<CameraTone> {
    let mut knots = [[0f32; 2]; 32];
    for k in knots.as_flattened_mut() {
        *k = r.f32()?;
    }
    let mut chroma = [0f32; CHROMA_N];
    for c in &mut chroma {
        *c = r.f32()?;
    }
    CameraTone::new(knots)?.with_chroma(chroma)
}

/// What [`write_cells`] wrote.
fn read_cells(r: &mut Reader<'_>) -> Option<Vec<Option<[f32; 3]>>> {
    let n = usize::from(r.u16()?);
    if n > MAX_CELLS {
        return None;
    }
    let seen = r.take(n.div_ceil(8))?;
    let mut cells = Vec::with_capacity(n);
    for i in 0..n {
        let present = seen.get(i / 8).is_some_and(|b| b & (1 << (i % 8)) != 0);
        cells.push(if present { Some(r.finite::<3>()?) } else { None });
    }
    Some(cells)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let out = self.bytes.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(out)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn finite<const N: usize>(&mut self) -> Option<[f32; N]> {
        let mut out = [0f32; N];
        for v in &mut out {
            *v = self.f32().filter(|v| v.is_finite())?;
        }
        Some(out)
    }
}

/// 64-bit FNV-1a: stable across builds and platforms.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for x in b {
            self.0 ^= u64::from(*x);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    fn text(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.bytes(s.as_bytes());
    }
}

fn fnv(b: &[u8]) -> u64 {
    let mut h = Fnv::new();
    h.bytes(b);
    h.0
}

#[cfg(test)]
mod tests;
