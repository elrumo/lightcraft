//! An append-only file of fixed-size records, and the key that names a photo in it.
//!
//! Layout: a 32-byte header (magic, record length, vector dimension, model id) and then records of
//! `rec_len` bytes, the first 16 of which are the photo's [`Key`]. Everything in the file is
//! recreatable, so nothing is fsynced; instead a crash can only leave a partial record at the
//! end, which [`RecordFile::open`] cuts off. Header fields are checked against what the caller
//! expects and never used to size an allocation.

use std::fs::{File, OpenOptions};
use std::hash::Hasher as _;
use std::io::{BufReader, Read, Write};
use std::path::Path;

use siphasher::sip128::{Hasher128 as _, SipHasher13};

/// Files larger than this are refused (a hostile or foreign file can't make a load read forever).
pub const MAX_FILE_BYTES: u64 = 2 << 30;
/// Bytes of the header.
pub const HEADER_LEN: usize = 32;
/// Bytes of a record's key.
pub const KEY_LEN: usize = 16;
/// Longest model id (ASCII), in bytes.
pub const MODEL_LEN: usize = 16;

const MAGIC: [u8; 8] = *b"LCVSTOR1";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Not a store file (or cut inside its header).
    #[error("not a vision store: {0}")]
    Format(&'static str),
    /// A store for another model, vector size or record layout: its contents are useless here.
    #[error("the store holds {found}, not {wanted}")]
    Mismatch { found: String, wanted: String },
    /// Data that must not be stored (wrong length, not finite, all zero…).
    #[error("invalid data: {0}")]
    Invalid(&'static str),
    /// The model's files are damaged, incomplete or of another kind.
    #[error("model: {0}")]
    Model(String),
    /// No model in this folder (it is downloaded only when the user asks).
    #[error("model files not found in {0}")]
    Missing(std::path::PathBuf),
    #[cfg(all(feature = "siglip", not(target_arch = "wasm32")))]
    #[error(transparent)]
    Candle(#[from] candle_core::Error),
}

/// What a photo's data is filed under: its 128-bit content hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(pub [u8; KEY_LEN]);

impl Key {
    /// The key of a photo's content key (`media::content_key`): a 32-digit content hash maps to
    /// itself, anything else (`demo:3`, `file:path:size`) to a hash of the text.
    pub fn of(content_key: &str) -> Key {
        match Key::from_hex(content_key) {
            Some(k) => k,
            None => {
                let mut h = SipHasher13::new_with_keys(0x4c56_5f4b_4559_5f31, 0x6f74_6865_725f_6b65);
                h.write(content_key.as_bytes());
                Key(h.finish128().as_u128().to_be_bytes())
            }
        }
    }

    /// Exactly 32 hex digits (what `Hash128` prints), else `None`.
    pub fn from_hex(s: &str) -> Option<Key> {
        if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u128::from_str_radix(s, 16).ok().map(|v| Key(v.to_be_bytes()))
    }

    pub fn to_hex(&self) -> String {
        format!("{:032x}", u128::from_be_bytes(self.0))
    }

    /// The key at the start of a record.
    pub fn read(rec: &[u8]) -> Option<Key> {
        rec.get(..KEY_LEN).and_then(|b| <[u8; KEY_LEN]>::try_from(b).ok()).map(Key)
    }
}

/// What a store must hold to be usable: the same model, vector size and record layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec<'a> {
    /// Model id, ASCII, at most [`MODEL_LEN`] bytes (`siglip2-b16-224`).
    pub model: &'a str,
    pub dim: usize,
    /// Bytes per record, key included.
    pub rec_len: usize,
}

impl Spec<'_> {
    /// Whether the model id and layout can be stored.
    pub fn check(&self) -> Result<(), Error> {
        if self.model.is_empty() || self.model.len() > MODEL_LEN || !self.model.is_ascii() || self.model.bytes().any(|b| b == 0) {
            return Err(Error::Invalid("model id"));
        }
        if self.dim == 0 || self.rec_len <= KEY_LEN || u32::try_from(self.dim).is_err() || u32::try_from(self.rec_len).is_err() {
            return Err(Error::Invalid("record layout"));
        }
        Ok(())
    }

    /// The header every file (and upload) of this layout starts with.
    pub fn header(&self) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        {
            let mut put = |at: usize, bytes: &[u8]| {
                if let Some(dst) = h.get_mut(at..at + bytes.len()) {
                    dst.copy_from_slice(bytes);
                }
            };
            put(0, &MAGIC);
            put(8, &u32::try_from(self.rec_len).unwrap_or(0).to_le_bytes());
            put(12, &u32::try_from(self.dim).unwrap_or(0).to_le_bytes());
            put(16, self.model.as_bytes().get(..MODEL_LEN).unwrap_or(self.model.as_bytes()));
        }
        h
    }

    /// Checks a header read from a file or an upload against this layout.
    pub fn check_header(&self, h: &[u8]) -> Result<(), Error> {
        self.check()?;
        let h = h.get(..HEADER_LEN).ok_or(Error::Format("short header"))?;
        if h.get(..8) != Some(&MAGIC[..]) {
            return Err(Error::Format("bad magic"));
        }
        let word = |at: usize| h.get(at..at + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_le_bytes).unwrap_or(0);
        let (rec_len, dim) = (word(8) as usize, word(12) as usize);
        let model_bytes = h.get(16..HEADER_LEN).unwrap_or(&[]);
        let model_bytes = model_bytes.split(|&b| b == 0).next().unwrap_or(&[]);
        let model = String::from_utf8_lossy(model_bytes).into_owned();
        if model != self.model || dim != self.dim || rec_len != self.rec_len {
            return Err(Error::Mismatch {
                found: format!("{model} (dim {dim}, {rec_len}-byte records)"),
                wanted: format!("{} (dim {}, {}-byte records)", self.model, self.dim, self.rec_len),
            });
        }
        Ok(())
    }
}

/// An open store file, appended to as records are added.
#[derive(Debug)]
pub struct RecordFile {
    file: File,
    rec_len: usize,
    /// Bytes in the file, header included (where a failed append rolls back to).
    len: u64,
}

impl RecordFile {
    /// Opens `path` (creating it with a header when absent or cut inside its header), checks the
    /// header against `spec`, cuts a partial record off the end, and calls `each` with every whole
    /// record in order. An unrelated, foreign or other-model file is an error, never overwritten.
    pub fn open(path: &Path, spec: &Spec<'_>, mut each: impl FnMut(&[u8])) -> Result<RecordFile, Error> {
        spec.check()?;
        let mut file = OpenOptions::new().read(true).append(true).create(true).open(path)?;
        let len = file.metadata()?.len();
        if len > MAX_FILE_BYTES {
            return Err(Error::Format("file too large"));
        }
        if len < HEADER_LEN as u64 {
            // new, or a header that was never fully written: nothing in it is worth keeping
            file.set_len(0)?;
            file.write_all(&spec.header())?;
            return Ok(RecordFile { file, rec_len: spec.rec_len, len: HEADER_LEN as u64 });
        }
        let mut reader = BufReader::new(&file);
        let mut header = [0u8; HEADER_LEN];
        reader.read_exact(&mut header)?;
        spec.check_header(&header)?;

        let body = len - HEADER_LEN as u64;
        let rec = spec.rec_len as u64;
        let whole = body / rec;
        let mut buf = vec![0u8; spec.rec_len];
        for _ in 0..whole {
            reader.read_exact(&mut buf)?;
            each(&buf);
        }
        drop(reader);
        let keep = HEADER_LEN as u64 + whole * rec;
        if keep != len {
            file.set_len(keep)?;
        }
        Ok(RecordFile { file, rec_len: spec.rec_len, len: keep })
    }

    /// Appends one or more whole records. A failed write is rolled back so the file stays aligned.
    pub fn append(&mut self, recs: &[u8]) -> Result<(), Error> {
        if recs.is_empty() || !recs.len().is_multiple_of(self.rec_len) {
            return Err(Error::Invalid("record length"));
        }
        if let Err(e) = self.file.write_all(recs) {
            let _ = self.file.set_len(self.len);
            return Err(e.into());
        }
        self.len += recs.len() as u64;
        Ok(())
    }
}
