//! Files handed to a command by content rather than by path: `files: [{name, data}]`, `data` the
//! file's bytes in base64 (standard alphabet, padded). The browser's file chooser gives no paths,
//! and an agent may pass a file it holds; commands that read files take this beside `paths`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

/// The parameter that lists files by content.
pub const KEY: &str = "files";

/// A file's name with no folders in it (`Pack/Look.xmp` → `Look.xmp`).
pub fn base_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// The `files` parameter for these files (name, bytes).
pub fn to_param<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Value {
    Value::Array(files.into_iter().map(|(name, bytes)| json!({"name": name, "data": STANDARD.encode(bytes)})).collect())
}

/// The files `p` lists under [`KEY`], each read or the reason it couldn't be (`(name, reason)`).
/// No such parameter: none.
pub fn from_params(p: &Value) -> Vec<Result<(String, Vec<u8>), (String, String)>> {
    let Some(list) = p.get(KEY).and_then(Value::as_array) else { return Vec::new() };
    list.iter()
        .enumerate()
        .map(|(i, f)| {
            let name = f.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
            let name = name.unwrap_or_else(|| format!("file {}", i + 1));
            let Some(data) = f.get("data").and_then(Value::as_str) else { return Err((name, "no data".into())) };
            // (whitespace a line-wrapping encoder left in is not an error)
            let data: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
            match STANDARD.decode(data.as_bytes()) {
                Ok(bytes) => Ok((name, bytes)),
                Err(e) => Err((name, format!("data is not base64: {e}"))),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip_and_bad_entries_say_why() {
        let p = json!({ "files": to_param([("Look.xmp", b"<x/>".as_slice()), ("empty.lcpreset", b"".as_slice())]) });
        let got = from_params(&p);
        assert_eq!(got, vec![Ok(("Look.xmp".to_string(), b"<x/>".to_vec())), Ok(("empty.lcpreset".to_string(), Vec::new()))]);
        let bad = json!({ "files": [{"name": "a.xmp", "data": "!!not base64!!"}, {"name": "b.xmp"}, {"data": "PHgv\nPg=="}, 7] });
        let got = from_params(&bad);
        assert!(matches!(&got[0], Err((n, why)) if n == "a.xmp" && why.contains("base64")), "{got:?}");
        assert!(matches!(&got[1], Err((n, why)) if n == "b.xmp" && why == "no data"), "{got:?}");
        assert_eq!(got[2], Ok(("file 3".to_string(), b"<x/>".to_vec())), "unnamed, and wrapped base64 reads");
        assert!(got[3].is_err());
        assert!(from_params(&json!({})).is_empty());
        assert!(from_params(&json!({ "files": "nope" })).is_empty());
        assert_eq!((base_name("Pack/Sub\\Look.xmp"), base_name("Look.xmp"), base_name("")), ("Look.xmp", "Look.xmp", ""));
    }
}
