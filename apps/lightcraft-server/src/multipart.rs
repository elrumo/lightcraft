//! `multipart/form-data` (RFC 7578), streamed: what a browser's file form sends (the **Add photos** form of an
//! album link, [`crate::guest`]). Each part's head is read (at most [`HEAD_MAX`] bytes) and its body handed on
//! in pieces as it arrives, never held whole, so a form of large photos takes little memory.
//!
//! Malformed input is an error, never a panic: a body that ends before its closing boundary, a part head that
//! is too long, a boundary that is missing or too long.

use std::io::Read;

/// The most bytes a part's head (its `Content-Disposition` and `Content-Type` lines) may take.
pub const HEAD_MAX: usize = 16 << 10;
const CHUNK: usize = 256 << 10;

/// A part's head: the form field it is, and the file name for a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Part {
    pub name: String,
    pub filename: Option<String>,
}

/// The boundary of a `Content-Type: multipart/form-data; boundary=…` (quoted or not; 1–70 characters).
pub fn boundary(content_type: &str) -> Option<String> {
    let mut params = content_type.split(';');
    if !params.next()?.trim().eq_ignore_ascii_case("multipart/form-data") {
        return None;
    }
    let b = params.find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim().eq_ignore_ascii_case("boundary").then(|| v.trim().trim_matches('"').to_string())
    })?;
    (!b.is_empty() && b.len() <= 70 && b.bytes().all(|c| c.is_ascii_graphic() || c == b' ')).then_some(b)
}

/// The parts of a form, one after the other: [`Multipart::next_part`], then [`Multipart::read_body`] (or
/// [`Multipart::read_text`]) before the next.
pub struct Multipart<'a> {
    r: &'a mut dyn Read,
    /// `\r\n--<boundary>`: what ends a part (the body starts with a `\r\n` of ours, so the first boundary
    /// matches too).
    delim: Vec<u8>,
    buf: Vec<u8>,
    /// What each read lands in.
    chunk: Vec<u8>,
    eof: bool,
    /// The body of the part handed out last hasn't been read.
    in_body: bool,
    done: bool,
    started: bool,
}

/// Where `needle` first is in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let first = *needle.first()?;
    let mut from = 0;
    while let Some(i) = hay.get(from..)?.iter().position(|&b| b == first) {
        let at = from + i;
        if hay.get(at..at + needle.len())? == needle {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

impl<'a> Multipart<'a> {
    pub fn new(r: &'a mut dyn Read, boundary: &str) -> Multipart<'a> {
        let mut delim = b"\r\n--".to_vec();
        delim.extend_from_slice(boundary.as_bytes());
        Multipart { r, delim, buf: b"\r\n".to_vec(), chunk: vec![0; CHUNK], eof: false, in_body: false, done: false, started: false }
    }

    /// Read more into the buffer; `false` at the end of the body.
    fn fill(&mut self) -> Result<bool, String> {
        if self.eof {
            return Ok(false);
        }
        loop {
            match self.r.read(&mut self.chunk) {
                Ok(n) => {
                    self.buf.extend_from_slice(self.chunk.get(..n).unwrap_or(&[]));
                    self.eof = n == 0;
                    return Ok(n > 0);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("reading the upload: {e}")),
            }
        }
    }

    /// Hand the body of the current part to `sink`, piece by piece, up to the next boundary.
    pub fn read_body(&mut self, sink: &mut dyn FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
        if !self.in_body {
            return Ok(());
        }
        loop {
            if let Some(i) = find(&self.buf, &self.delim) {
                sink(self.buf.get(..i).unwrap_or(&[]))?;
                self.buf.drain(..i + self.delim.len());
                self.in_body = false;
                return Ok(());
            }
            // all but what could be the start of a boundary
            let keep = self.delim.len().saturating_sub(1).min(self.buf.len());
            let send = self.buf.len() - keep;
            if send > 0 {
                sink(self.buf.get(..send).unwrap_or(&[]))?;
                self.buf.drain(..send);
            }
            if !self.fill()? {
                return Err("the upload ended in the middle of a file".into());
            }
        }
    }

    /// The body of the current part as text, if it is at most `max` bytes (a form field).
    pub fn read_text(&mut self, max: usize) -> Result<String, String> {
        let mut out = Vec::new();
        self.read_body(&mut |b| {
            if out.len() + b.len() > max {
                return Err(format!("a form field is longer than {max} bytes"));
            }
            out.extend_from_slice(b);
            Ok(())
        })?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// The next part's head (`None`: that was the last). Skips the body of the part before, if it wasn't read.
    pub fn next_part(&mut self) -> Result<Option<Part>, String> {
        if self.done {
            return Ok(None);
        }
        if !self.started {
            // (anything before the first boundary is a preamble)
            self.started = true;
            self.in_body = true;
        }
        self.read_body(&mut |_| Ok(()))?;
        // after a boundary: `--` ends the form, a line break starts a part
        while self.buf.len() < 2 {
            if !self.fill()? {
                return Err("the upload ended after a boundary".into());
            }
        }
        if self.buf.starts_with(b"--") {
            self.done = true;
            return Ok(None);
        }
        loop {
            if let Some(i) = find(&self.buf, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(self.buf.get(..i).unwrap_or(&[])).into_owned();
                self.buf.drain(..i + 4);
                self.in_body = true;
                return Ok(Some(parse_head(&head)));
            }
            if self.buf.len() > HEAD_MAX {
                return Err("a part of the upload has too long a head".into());
            }
            if !self.fill()? {
                return Err("the upload ended in a part's head".into());
            }
        }
    }
}

/// `name` and `filename` of the `Content-Disposition` line of a part's head.
fn parse_head(head: &str) -> Part {
    let mut part = Part::default();
    for line in head.split("\r\n") {
        let Some((k, v)) = line.split_once(':') else { continue };
        if !k.trim().eq_ignore_ascii_case("content-disposition") {
            continue;
        }
        for (key, value) in params(v) {
            match key.to_ascii_lowercase().as_str() {
                "name" => part.name = value,
                "filename" => part.filename = Some(value),
                _ => {}
            }
        }
    }
    part
}

/// `form-data; name="photos"; filename="a;b.jpg"` → its parameters (quoted values may hold `;`, and `\"`).
fn params(v: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = v.chars().peekable();
    loop {
        // the key
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            chars.next();
            match c {
                '=' => break,
                ';' => key.clear(),
                c => key.push(c),
            }
        }
        if chars.peek().is_none() {
            return out;
        }
        let mut value = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => value.extend(chars.next()),
                    '"' => break,
                    c => value.push(c),
                }
            }
            // (to the next `;`)
            for c in chars.by_ref() {
                if c == ';' {
                    break;
                }
            }
        } else {
            for c in chars.by_ref() {
                if c == ';' {
                    break;
                }
                value.push(c);
            }
        }
        out.push((key.trim().to_string(), value.trim().to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(boundary: &str, parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
        let mut b = b"preamble to ignore".to_vec();
        for (name, file, data) in parts {
            b.extend(format!("\r\n--{boundary}\r\n").as_bytes());
            match file {
                Some(f) => b.extend(
                    format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: image/jpeg\r\n\r\n").as_bytes(),
                ),
                None => b.extend(format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes()),
            }
            b.extend_from_slice(data);
        }
        b.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
        b
    }

    /// Reads at most `n` bytes at a time (the boundary arrives in pieces).
    struct Trickle<'a>(&'a [u8], usize);
    impl Read for Trickle<'_> {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let n = self.0.len().min(self.1).min(out.len());
            out[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    fn parse(body: &[u8], boundary: &str, step: usize) -> Result<Vec<(Part, Vec<u8>)>, String> {
        let mut r = Trickle(body, step);
        let mut m = Multipart::new(&mut r, boundary);
        let mut out = Vec::new();
        while let Some(p) = m.next_part()? {
            let mut data = Vec::new();
            m.read_body(&mut |b| {
                data.extend_from_slice(b);
                Ok(())
            })?;
            out.push((p, data));
        }
        Ok(out)
    }

    #[test]
    fn parts_and_their_bodies() {
        let boundary = "----WebKitFormBoundaryAbC123";
        // a body holding a near-boundary, and line breaks at the edges
        let tricky = format!("\r\n--{}x\r\n\r\n--", &boundary[..10]).into_bytes();
        let photo: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
        let body = form(
            boundary,
            &[("name", None, b"Ann"), ("photos", Some("IMG_1.JPG"), &photo), ("photos", Some("odd.bin"), &tricky), ("photos", Some(""), b"")],
        );
        for step in [1, 3, 70, 4096, 1 << 20] {
            let parts = parse(&body, boundary, step).unwrap();
            assert_eq!(parts.len(), 4, "step {step}");
            assert_eq!(parts[0], (Part { name: "name".into(), filename: None }, b"Ann".to_vec()));
            assert_eq!(parts[1].0.filename.as_deref(), Some("IMG_1.JPG"));
            assert_eq!(parts[1].1, photo);
            assert_eq!(parts[2].1, tricky);
            assert_eq!((parts[3].0.filename.as_deref(), parts[3].1.len()), (Some(""), 0));
        }
    }

    #[test]
    fn malformed_forms_are_errors() {
        let boundary = "xyz";
        let body = form(boundary, &[("photos", Some("a.jpg"), b"data")]);
        // cut anywhere before the closing boundary
        for cut in [0, 5, 30, 60, body.len() - 8] {
            assert!(parse(&body[..cut], boundary, 7).is_err(), "cut at {cut}");
        }
        // a head that never ends
        let mut long = b"\r\n--xyz\r\nContent-Disposition: form-data; name=\"a\"".to_vec();
        long.extend(vec![b'x'; HEAD_MAX * 2]);
        assert!(parse(&long, boundary, 4096).is_err());
        // no boundary at all: the whole body is a preamble that never ends
        assert!(parse(b"just some bytes", boundary, 4).is_err());
        // a field too long for text
        let body = form(boundary, &[("name", None, &[b'a'; 100])]);
        let mut r = Trickle(&body, 1000);
        let mut m = Multipart::new(&mut r, boundary);
        m.next_part().unwrap();
        assert!(m.read_text(10).is_err());
    }

    #[test]
    fn heads_and_boundaries() {
        assert_eq!(boundary("multipart/form-data; boundary=----abc"), Some("----abc".into()));
        assert_eq!(boundary("Multipart/Form-Data; charset=utf-8; boundary=\"a b\""), Some("a b".into()));
        assert_eq!(boundary("application/json"), None);
        assert_eq!(boundary("multipart/form-data"), None);
        assert_eq!(boundary(&format!("multipart/form-data; boundary={}", "x".repeat(71))), None);
        let p = parse_head("Content-Disposition: form-data; name=\"photos\"; filename=\"a;b \\\"c\\\".jpg\"\r\nContent-Type: image/jpeg");
        assert_eq!(p, Part { name: "photos".into(), filename: Some("a;b \"c\".jpg".into()) });
        let p = parse_head("content-disposition: form-data; name=title");
        assert_eq!(p, Part { name: "title".into(), filename: None });
        assert_eq!(parse_head("garbage"), Part::default());
    }
}
