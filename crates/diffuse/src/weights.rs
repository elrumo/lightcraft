//! Reading `.safetensors` weights a tensor at a time.
//!
//! candle's own readers either map the file into memory (which needs `unsafe` here) or take the
//! whole file as one buffer, which for a 5 GB UNet means holding the file and its converted copy
//! at once. This reader parses the header, checks every tensor's offsets and size against the
//! file, and reads, converts and places one tensor at a time: the peak is the converted model plus
//! one tensor. [`Weights::load`] can take only the tensors whose names pass a filter (a layer of
//! an encoder that is run one layer at a time).
//!
//! The file is hostile until proven otherwise: a header that lies about sizes or offsets, a dtype
//! this crate doesn't know, a shape whose element count overflows, or a truncated file is an
//! error, never a panic or an allocation sized by the file.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// Largest header read (real ones are well under 1 MB).
const MAX_HEADER: u64 = 32 << 20;
/// Most tensors in one file, and most dimensions per tensor.
const MAX_TENSORS: usize = 100_000;
const MAX_DIMS: usize = 8;

fn bad(path: &Path, why: impl std::fmt::Display) -> Error {
    Error::Model(format!("{} is damaged or not the expected file ({why}): delete it in Settings → AI Models and download it again", name_of(path)))
}

fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// One tensor's place in the file.
#[derive(Clone, Debug)]
struct Entry {
    dtype: DType,
    shape: Vec<usize>,
    /// Byte range within the data section.
    start: u64,
    end: u64,
}

/// A `.safetensors` file whose header has been read and checked.
pub struct Weights {
    path: PathBuf,
    /// Where the data section starts.
    base: u64,
    entries: HashMap<String, Entry>,
}

impl Weights {
    /// Read and check `path`'s header.
    pub fn open(path: &Path) -> Result<Weights> {
        let mut f = File::open(path).map_err(|e| Error::Model(format!("can't open {}: {e}", name_of(path))))?;
        let len = f.metadata().map_err(|e| bad(path, e))?.len();
        let mut n8 = [0u8; 8];
        f.read_exact(&mut n8).map_err(|_| bad(path, "too short"))?;
        let n = u64::from_le_bytes(n8);
        let base = n.checked_add(8).filter(|b| n > 1 && n <= MAX_HEADER && *b <= len).ok_or_else(|| bad(path, "bad header size"))?;
        let mut head = vec![0u8; usize::try_from(n).map_err(|_| bad(path, "bad header size"))?];
        f.read_exact(&mut head).map_err(|_| bad(path, "truncated header"))?;
        let v: serde_json::Value = serde_json::from_slice(&head).map_err(|e| bad(path, e))?;
        let obj = v.as_object().ok_or_else(|| bad(path, "the header isn't a table"))?;
        if obj.len() > MAX_TENSORS {
            return Err(bad(path, "too many tensors"));
        }
        let data = len - base;
        let mut entries = HashMap::with_capacity(obj.len());
        for (name, t) in obj {
            if name == "__metadata__" {
                continue;
            }
            let dtype = match t.get("dtype").and_then(|d| d.as_str()) {
                Some("F16") => DType::F16,
                Some("BF16") => DType::BF16,
                Some("F32") => DType::F32,
                Some("F64") => DType::F64,
                // (buffers such as a text encoder's position ids)
                Some("I64") => DType::I64,
                Some("U32") => DType::U32,
                Some("U8") => DType::U8,
                other => return Err(bad(path, format!("{name}: dtype {other:?} isn't supported"))),
            };
            let dims = t.get("shape").and_then(|s| s.as_array()).ok_or_else(|| bad(path, format!("{name}: no shape")))?;
            if dims.len() > MAX_DIMS {
                return Err(bad(path, format!("{name}: too many dimensions")));
            }
            let shape: Vec<usize> = dims
                .iter()
                .map(|d| d.as_u64().and_then(|d| usize::try_from(d).ok()))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| bad(path, format!("{name}: bad shape")))?;
            let count = shape.iter().try_fold(1usize, |a, d| a.checked_mul(*d)).ok_or_else(|| bad(path, format!("{name}: shape too large")))?;
            let bytes = count.checked_mul(dtype.size_in_bytes()).ok_or_else(|| bad(path, format!("{name}: shape too large")))?;
            let offs = t.get("data_offsets").and_then(|o| o.as_array()).ok_or_else(|| bad(path, format!("{name}: no offsets")))?;
            let (Some(start), Some(end)) = (offs.first().and_then(|v| v.as_u64()), offs.get(1).and_then(|v| v.as_u64())) else {
                return Err(bad(path, format!("{name}: bad offsets")));
            };
            if offs.len() != 2 || start > end || end > data || end - start != bytes as u64 {
                return Err(bad(path, format!("{name}: its offsets don't match its size or the file")));
            }
            entries.insert(name.clone(), Entry { dtype, shape, start, end });
        }
        Ok(Weights { path: path.to_path_buf(), base, entries })
    }

    /// The names of the tensors in the file.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// The shape of tensor `name`, if the file has it.
    pub fn shape(&self, name: &str) -> Option<&[usize]> {
        self.entries.get(name).map(|e| e.shape.as_slice())
    }

    /// Read the tensors whose names pass `keep`, as `dtype` on `device`, one at a time (in file
    /// order, so the reads are sequential).
    pub fn load(&self, keep: &dyn Fn(&str) -> bool, dtype: DType, device: &Device) -> Result<HashMap<String, Tensor>> {
        let mut todo: Vec<(&String, &Entry)> = self.entries.iter().filter(|(n, _)| keep(n)).collect();
        todo.sort_by_key(|(_, e)| e.start);
        let mut f = File::open(&self.path).map_err(|e| Error::Model(format!("can't open {}: {e}", name_of(&self.path))))?;
        let mut out = HashMap::with_capacity(todo.len());
        let mut buf = Vec::new();
        for (name, e) in todo {
            let n = usize::try_from(e.end - e.start).map_err(|_| bad(&self.path, "tensor too large"))?;
            buf.clear();
            buf.try_reserve_exact(n).map_err(|_| Error::Model(format!("not enough memory to load {}", name_of(&self.path))))?;
            buf.resize(n, 0);
            let at = self.base.checked_add(e.start).ok_or_else(|| bad(&self.path, "bad offsets"))?;
            f.seek(SeekFrom::Start(at)).map_err(|err| bad(&self.path, err))?;
            f.read_exact(&mut buf).map_err(|_| bad(&self.path, "truncated"))?;
            let t = Tensor::from_raw_buffer(&buf, e.dtype, &e.shape, &Device::Cpu)?;
            let t = if t.dtype() == dtype { t } else { t.to_dtype(dtype)? };
            out.insert(name.clone(), t.to_device(device)?);
        }
        Ok(out)
    }

    /// All the tensors as a [`VarBuilder`] (`dtype` on `device`).
    pub fn var_builder(&self, dtype: DType, device: &Device) -> Result<VarBuilder<'static>> {
        Ok(VarBuilder::from_tensors(self.load(&|_| true, dtype, device)?, dtype, device))
    }

    /// The tensors whose names start with `prefix` as a [`VarBuilder`] rooted at the prefix.
    pub fn var_builder_of(&self, prefix: &str, dtype: DType, device: &Device) -> Result<VarBuilder<'static>> {
        let ts = self.load(&|n| n.starts_with(prefix), dtype, device)?;
        let ts = ts.into_iter().filter_map(|(n, t)| n.strip_prefix(prefix).map(|s| (s.to_string(), t))).collect();
        Ok(VarBuilder::from_tensors(ts, dtype, device))
    }
}

/// Whether `path` is exactly the pinned file: its size, then its SHA-256. The hash of a file that
/// passed is remembered for the rest of the run (with its size and modification time), so a model
/// of several GB is hashed once per launch, not once per Generate.
pub fn verify(path: &Path, size: u64, sha256: &str) -> Result<()> {
    use std::sync::{Mutex, OnceLock, PoisonError};
    static SEEN: OnceLock<Mutex<Vec<(PathBuf, u64, Option<std::time::SystemTime>)>>> = OnceLock::new();
    let meta = std::fs::metadata(path).map_err(|_| Error::Model(format!("{} is missing", name_of(path))))?;
    if meta.len() != size {
        return Err(bad(path, format!("{} bytes, not {size}", meta.len())));
    }
    let key = (path.to_path_buf(), size, meta.modified().ok());
    let seen = SEEN.get_or_init(|| Mutex::new(Vec::new()));
    if seen.lock().unwrap_or_else(PoisonError::into_inner).contains(&key) {
        return Ok(());
    }
    let mut f = File::open(path).map_err(|e| Error::Model(format!("can't open {}: {e}", name_of(path))))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| bad(path, e))?;
        if n == 0 {
            break;
        }
        h.update(buf.get(..n).unwrap_or_default());
    }
    let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if got != sha256 {
        return Err(bad(path, "its SHA-256 doesn't match"));
    }
    seen.lock().unwrap_or_else(PoisonError::into_inner).push(key);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A safetensors file with `tensors` (name, dtype, shape, raw bytes), written by hand.
    pub(crate) fn write(path: &Path, tensors: &[(&str, &str, Vec<usize>, Vec<u8>)]) {
        let mut head = serde_json::Map::new();
        let mut data = Vec::new();
        for (name, dtype, shape, bytes) in tensors {
            let start = data.len();
            data.extend_from_slice(bytes);
            head.insert(name.to_string(), serde_json::json!({"dtype": dtype, "shape": shape, "data_offsets": [start, data.len()]}));
        }
        let head = serde_json::to_vec(&serde_json::Value::Object(head)).unwrap();
        let mut out = (head.len() as u64).to_le_bytes().to_vec();
        out.extend(head);
        out.extend(data);
        std::fs::write(path, out).unwrap();
    }

    fn f32s(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-diffuse-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join("w.safetensors")
    }

    #[test]
    fn reads_converts_and_filters_tensors() {
        let p = tmp("ok");
        // 1.5 and -2.0 as little-endian f16
        let half = vec![0x00, 0x3e, 0x00, 0xc0];
        write(&p, &[("a.w", "F32", vec![2, 2], f32s(&[1.0, 2.0, 3.0, 4.0])), ("b.w", "F16", vec![2], half)]);
        let w = Weights::open(&p).unwrap();
        assert_eq!(w.shape("a.w"), Some(&[2usize, 2][..]));
        let all = w.load(&|_| true, DType::F32, &Device::Cpu).unwrap();
        assert_eq!(all["a.w"].to_vec2::<f32>().unwrap(), vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
        assert_eq!(all["b.w"].to_vec1::<f32>().unwrap(), vec![1.5, -2.0]);
        let some = w.load(&|n| n.starts_with("b."), DType::F16, &Device::Cpu).unwrap();
        assert_eq!(some.len(), 1);
        assert_eq!(some["b.w"].dtype(), DType::F16);
        let vb = w.var_builder_of("a.", DType::F32, &Device::Cpu).unwrap();
        assert!(vb.get((2, 2), "w").is_ok());
    }

    #[test]
    fn hostile_files_are_errors_not_panics() {
        let p = tmp("bad");
        for bytes in [
            vec![],
            vec![1, 2, 3],
            u64::MAX.to_le_bytes().to_vec(),
            [&8u64.to_le_bytes()[..], b"not json"].concat(),
            [&2u64.to_le_bytes()[..], b"[]"].concat(),
        ] {
            std::fs::write(&p, bytes).unwrap();
            assert!(Weights::open(&p).is_err());
        }
        // offsets past the end, a size that lies, an unknown dtype, a huge shape
        let cases: Vec<serde_json::Value> = vec![
            serde_json::json!({"a": {"dtype": "F32", "shape": [4], "data_offsets": [0, 400]}}),
            serde_json::json!({"a": {"dtype": "F32", "shape": [4], "data_offsets": [0, 8]}}),
            serde_json::json!({"a": {"dtype": "Q4", "shape": [4], "data_offsets": [0, 16]}}),
            serde_json::json!({"a": {"dtype": "F32", "shape": [u64::MAX, 2], "data_offsets": [0, 16]}}),
            serde_json::json!({"a": {"dtype": "F32", "shape": [4], "data_offsets": [16, 0]}}),
            serde_json::json!({"a": {"dtype": "F32", "shape": [4]}}),
        ];
        for c in cases {
            let head = serde_json::to_vec(&c).unwrap();
            let mut out = (head.len() as u64).to_le_bytes().to_vec();
            out.extend(head);
            out.extend([0u8; 16]);
            std::fs::write(&p, out).unwrap();
            assert!(Weights::open(&p).is_err(), "{c}");
        }
        assert!(Weights::open(Path::new("/nonexistent/w.safetensors")).is_err());
    }

    #[test]
    fn verify_checks_size_then_hash_and_remembers() {
        let p = tmp("verify");
        std::fs::write(&p, b"abc").unwrap();
        let sha = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify(&p, 4, sha).is_err());
        assert!(verify(&p, 3, &"0".repeat(64)).is_err());
        assert!(verify(&p, 3, sha).is_ok());
        assert!(verify(&p, 3, sha).is_ok());
        assert!(verify(Path::new("/nonexistent"), 3, sha).is_err());
    }
}
