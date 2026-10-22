//! A small, read-only MessagePack decoder, for Lightroom (cloud) catalog documents.
//!
//! Clean-room from the public MessagePack specification
//! (<https://github.com/msgpack/msgpack/blob/master/spec.md>). Input is untrusted: every length is
//! checked against the bytes left before anything is read or allocated, and nesting is capped.
//! Binary values of printable text become strings, other binary and extension values `null`; map keys that aren't strings are written as JSON.

use serde_json::{Map, Number, Value};

const MAX_DEPTH: usize = 64;

/// Decode one MessagePack value. Bytes after it are ignored.
pub fn decode(bytes: &[u8]) -> Result<Value, String> {
    Reader { bytes, at: 0 }.value(0)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn left(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).ok_or("MessagePack length overflow")?;
        let out = self.bytes.get(self.at..end).ok_or("truncated MessagePack")?;
        self.at = end;
        Ok(out)
    }

    /// A big-endian unsigned integer of `n` (1, 2, 4 or 8) bytes.
    fn uint(&mut self, n: usize) -> Result<u64, String> {
        Ok(self.take(n)?.iter().fold(0, |acc, b| (acc << 8) | u64::from(*b)))
    }

    fn len(&mut self, n: usize) -> Result<usize, String> {
        usize::try_from(self.uint(n)?).map_err(|_| "MessagePack length is too large".into())
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err("MessagePack nesting is too deep".into());
        }
        let tag = *self.take(1)?.first().ok_or("truncated MessagePack")?;
        Ok(match tag {
            0x00..=0x7f => Value::from(tag),
            0x80..=0x8f => self.map(usize::from(tag & 0x0f), depth)?,
            0x90..=0x9f => self.array(usize::from(tag & 0x0f), depth)?,
            0xa0..=0xbf => self.text(usize::from(tag & 0x1f))?,
            0xc0 => Value::Null,
            0xc1 => return Err("invalid MessagePack tag 0xc1".into()),
            0xc2 => Value::Bool(false),
            0xc3 => Value::Bool(true),
            // bin 8 / 16 / 32
            0xc4..=0xc6 => {
                let n = self.len(1 << (tag - 0xc4))?;
                let bytes = self.take(n)?;
                // Lightroom stores ids and hashes as hex text in a bin: those read as strings
                match std::str::from_utf8(bytes) {
                    Ok(s) if s.len() <= 256 && s.bytes().all(|b| b.is_ascii_graphic()) => Value::String(s.to_string()),
                    _ => Value::Null,
                }
            }
            // ext 8 / 16 / 32: length, type byte, data
            0xc7..=0xc9 => {
                let n = self.len(1 << (tag - 0xc7))?;
                self.take(n.checked_add(1).ok_or("MessagePack length overflow")?)?;
                Value::Null
            }
            0xca => {
                let bits = u32::try_from(self.uint(4)?).map_err(|_| "invalid float")?;
                Number::from_f64(f64::from(f32::from_bits(bits))).map_or(Value::Null, Value::Number)
            }
            0xcb => Number::from_f64(f64::from_bits(self.uint(8)?)).map_or(Value::Null, Value::Number),
            0xcc..=0xcf => Value::from(self.uint(1 << (tag - 0xcc))?),
            0xd0..=0xd3 => {
                let n = 1usize << (tag - 0xd0);
                let shift = 64 - 8 * n as u32;
                Value::from(((self.uint(n)? << shift) as i64) >> shift)
            }
            // fixext 1 / 2 / 4 / 8 / 16: type byte, data
            0xd4..=0xd8 => {
                self.take(1 + (1 << (tag - 0xd4)))?;
                Value::Null
            }
            0xd9..=0xdb => {
                let n = self.len(1 << (tag - 0xd9))?;
                self.text(n)?
            }
            0xdc | 0xdd => {
                let n = self.len(if tag == 0xdc { 2 } else { 4 })?;
                self.array(n, depth)?
            }
            0xde | 0xdf => {
                let n = self.len(if tag == 0xde { 2 } else { 4 })?;
                self.map(n, depth)?
            }
            0xe0..=0xff => Value::from(tag as i8),
        })
    }

    fn text(&mut self, n: usize) -> Result<Value, String> {
        Ok(Value::String(String::from_utf8_lossy(self.take(n)?).into_owned()))
    }

    fn array(&mut self, n: usize, depth: usize) -> Result<Value, String> {
        // every value takes at least one byte, so a longer count can't be real
        if n > self.left() {
            return Err("truncated MessagePack array".into());
        }
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(self.value(depth + 1)?);
        }
        Ok(Value::Array(out))
    }

    fn map(&mut self, n: usize, depth: usize) -> Result<Value, String> {
        if n > self.left() / 2 {
            return Err("truncated MessagePack map".into());
        }
        let mut out = Map::new();
        for _ in 0..n {
            let key = match self.value(depth + 1)? {
                Value::String(key) => key,
                other => other.to_string(),
            };
            out.insert(key, self.value(depth + 1)?);
        }
        Ok(Value::Object(out))
    }
}

/// MessagePack for `value` (strings, integers, booleans, null, arrays and maps): test fixtures.
#[cfg(test)]
pub(crate) fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    let put = |tag: u8, small: u8, len: usize, out: &mut Vec<u8>| {
        if len < if small == 0xa0 { 32 } else { 16 } {
            out.push(small | len as u8);
        } else {
            out.push(tag);
            out.extend((len as u32).to_be_bytes());
        }
    };
    match value {
        Value::Null => out.push(0xc0),
        Value::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
        Value::Number(n) => match (n.as_u64(), n.as_i64()) {
            (Some(n), _) => {
                out.push(0xcf);
                out.extend(n.to_be_bytes());
            }
            (_, Some(n)) => {
                out.push(0xd3);
                out.extend(n.to_be_bytes());
            }
            _ => {
                out.push(0xcb);
                out.extend(n.as_f64().unwrap_or(0.0).to_be_bytes());
            }
        },
        Value::String(s) => {
            put(0xdb, 0xa0, s.len(), &mut out);
            out.extend(s.as_bytes());
        }
        Value::Array(items) => {
            put(0xdd, 0x90, items.len(), &mut out);
            for item in items {
                out.extend(encode(item));
            }
        }
        Value::Object(map) => {
            put(0xdf, 0x80, map.len(), &mut out);
            for (k, v) in map {
                out.extend(encode(&Value::String(k.clone())));
                out.extend(encode(v));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn documents_round_trip() {
        let doc = json!({"type": "album", "name": "Trip — 2024", "parent": {"id": "a1"}, "n": -5, "big": 70000, "ok": true,
            "list": [1, null, "x"], "long": "y".repeat(40), "f": 1.5});
        assert_eq!(decode(&encode(&doc)).unwrap(), doc);
        // fixed-width forms another writer would use
        assert_eq!(decode(&[0xd0, 0xfe]).unwrap(), json!(-2));
        assert_eq!(decode(&[0xcd, 0x01, 0x00]).unwrap(), json!(256));
        assert_eq!(decode(&[0xca, 0x3f, 0xc0, 0, 0]).unwrap(), json!(1.5));
        assert_eq!(decode(&[0x82, 0x01, 0xa1, b'a', 0xc4, 2, 9, 9, 0xc0]).unwrap(), json!({"1": "a", "null": null}));
        // a bin of printable text (an id, a hash) is a string; other bytes are null
        assert_eq!(decode(&[0xc4, 3, b'a', b'b', b'c']).unwrap(), json!("abc"));
        assert_eq!(decode(&[0xc4, 2, 0xff, 0x00]).unwrap(), Value::Null);
    }

    /// Hostile input is an error, never a panic or an allocation sized by a forged length.
    #[test]
    fn hostile_input_is_an_error() {
        let doc = encode(&json!({"name": "a long enough name", "list": [1, 2, 3]}));
        for end in 0..doc.len() {
            assert!(decode(&doc[..end]).is_err(), "truncated at {end}");
        }
        assert!(decode(&[0xdd, 0xff, 0xff, 0xff, 0xff]).is_err());
        assert!(decode(&[0xdf, 0x7f, 0xff, 0xff, 0xff, 0xc0]).is_err());
        assert!(decode(&[0xdb, 0xff, 0xff, 0xff, 0xff, b'a']).is_err());
        assert!(decode(&[0xc6, 0xff, 0xff, 0xff, 0xff]).is_err());
        assert!(decode(&[0xc1]).is_err());
        assert!(decode(&[0x91; 200]).is_err());
        assert!(decode(&[]).is_err());
    }
}
