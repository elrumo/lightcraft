//! Little-endian reader and writer for the compact data files (`places.bin`, `land.bin`).
//! Every read is bounds-checked: a damaged or truncated file is an error, never a panic.

use miniz_oxide::inflate::decompress_to_vec_with_limit;

/// The data file is damaged, truncated or from another version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataError(pub &'static str);

impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "place data: {}", self.0)
    }
}

impl std::error::Error for DataError {}

/// Largest payload we inflate (the real files are a few MB): a corrupt length can't exhaust memory.
const MAX_INFLATED: usize = 64 << 20;

pub fn inflate(data: &[u8]) -> Result<Vec<u8>, DataError> {
    decompress_to_vec_with_limit(data, MAX_INFLATED).map_err(|_| DataError("can't decompress"))
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DataError> {
        let end = self.pos.checked_add(n).ok_or(DataError("length overflow"))?;
        let s = self.buf.get(self.pos..end).ok_or(DataError("truncated"))?;
        self.pos = end;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, DataError> {
        Ok(self.take(1)?.first().copied().unwrap_or(0))
    }

    pub fn u16(&mut self) -> Result<u16, DataError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b.first().copied().unwrap_or(0), b.get(1).copied().unwrap_or(0)]))
    }

    pub fn i16(&mut self) -> Result<i16, DataError> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32, DataError> {
        let b = self.take(4)?;
        let mut a = [0u8; 4];
        a.copy_from_slice(b);
        Ok(u32::from_le_bytes(a))
    }

    pub fn i32(&mut self) -> Result<i32, DataError> {
        Ok(self.u32()? as i32)
    }

    /// LEB128, at most 5 bytes (a `u32`).
    pub fn varint(&mut self) -> Result<u32, DataError> {
        let mut v: u32 = 0;
        for shift in (0..35).step_by(7) {
            let b = self.u8()?;
            v |= u32::from(b & 0x7f).checked_shl(shift).ok_or(DataError("varint"))?;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(DataError("varint too long"))
    }

    /// A length-prefixed UTF-8 string, as a span of the buffer.
    pub fn str_span(&mut self) -> Result<&'a str, DataError> {
        let n = self.varint()? as usize;
        std::str::from_utf8(self.take(n)?).map_err(|_| DataError("not UTF-8"))
    }

    /// An element count that must fit what's left (each element takes at least `min_size` bytes).
    pub fn count(&mut self, min_size: usize) -> Result<usize, DataError> {
        let n = self.u32()? as usize;
        if n.checked_mul(min_size.max(1)).is_none_or(|b| b > self.buf.len().saturating_sub(self.pos)) {
            return Err(DataError("implausible count"));
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_input_is_an_error() {
        let mut r = Reader::new(&[1, 2]);
        assert!(r.u32().is_err());
        let mut r = Reader::new(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
        assert!(r.varint().is_err());
        let mut r = Reader::new(&[0xff, 0xff, 0xff, 0xff, 0x0f]);
        assert_eq!(r.varint(), Ok(u32::MAX));
        let mut r = Reader::new(&[5, b'a']);
        assert!(r.str_span().is_err());
    }

    #[test]
    fn counts_are_checked_against_what_is_left() {
        let mut r = Reader::new(&[0xff, 0xff, 0xff, 0x7f, 0, 0]);
        assert!(r.count(4).is_err());
    }
}
