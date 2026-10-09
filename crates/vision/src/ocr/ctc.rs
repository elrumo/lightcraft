//! The recogniser's output: CTC decoding, and the character dictionary that ships beside the model
//! (`inference.yml`, a YAML list we read with a tiny parser instead of a YAML dependency).

use crate::Error;

/// Most characters a dictionary may have (the real one has ~18.7k).
const MAX_DICT: usize = 200_000;
/// Longest dictionary entry, in bytes (a character, or an emoji sequence).
const MAX_ENTRY: usize = 32;

/// The `character_dict:` list of an `inference.yml`.
pub fn parse_dict(yml: &str) -> Result<Vec<String>, Error> {
    let bad = |m: &str| Error::Model(format!("the recogniser's dictionary: {m}"));
    let mut lines = yml.lines().skip_while(|l| l.trim() != "character_dict:");
    lines.next().ok_or_else(|| bad("no `character_dict:`"))?;
    let mut out = Vec::new();
    for line in lines {
        // (the list sits at the key's own indentation: `  - x`)
        let Some(item) = line.strip_prefix("  - ").or_else(|| line.strip_prefix("- ")) else { break };
        if out.len() >= MAX_DICT {
            return Err(bad("too many entries"));
        }
        let s = scalar(item).ok_or_else(|| bad(&format!("can't read the entry {item:?}")))?;
        if s.is_empty() || s.len() > MAX_ENTRY {
            return Err(bad("an entry is empty or too long"));
        }
        out.push(s);
    }
    if out.is_empty() {
        return Err(bad("it is empty"));
    }
    Ok(out)
}

/// A YAML scalar: plain, `'single'` or `"double"` quoted.
fn scalar(s: &str) -> Option<String> {
    if let Some(inner) = s.strip_prefix('\'') {
        return Some(inner.strip_suffix('\'')?.replace("''", "'"));
    }
    if let Some(inner) = s.strip_prefix('"') {
        let inner = inner.strip_suffix('"')?;
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                ' ' => out.push(' '),
                'x' => out.push(hex(&mut chars, 2)?),
                'u' => out.push(hex(&mut chars, 4)?),
                'U' => out.push(hex(&mut chars, 8)?),
                _ => return None,
            }
        }
        return Some(out);
    }
    Some(s.to_string())
}

fn hex(chars: &mut std::str::Chars<'_>, n: usize) -> Option<char> {
    let mut v: u32 = 0;
    for _ in 0..n {
        v = v.checked_mul(16)?.checked_add(chars.next()?.to_digit(16)?)?;
    }
    char::from_u32(v)
}

/// Decodes one line's `steps × classes` probabilities: the most likely class per step, repeats
/// merged, blanks (class 0) dropped. Class `i ≥ 1` is `dict[i - 1]`, the class after the
/// dictionary is a space. Returns the text and the mean probability of the characters kept
/// (0 for no text).
pub fn decode(probs: &[f32], steps: usize, classes: usize, dict: &[String]) -> (String, f32) {
    let (mut text, mut sum, mut kept, mut last) = (String::new(), 0f32, 0u32, 0usize);
    if classes == 0 {
        return (text, 0.0);
    }
    for t in 0..steps {
        let Some(row) = probs.get(t * classes..(t + 1) * classes) else { break };
        let (best, p) = row.iter().enumerate().fold((0usize, f32::MIN), |(bi, bp), (i, &p)| if p > bp { (i, p) } else { (bi, bp) });
        if best != 0 && best != last {
            let piece = match best.checked_sub(1).and_then(|i| dict.get(i)) {
                Some(c) => Some(c.as_str()),
                None if best == dict.len() + 1 => Some(" "),
                None => None,
            };
            if let Some(c) = piece {
                text.push_str(c);
                sum += p;
                kept += 1;
            }
        }
        last = best;
    }
    (text, if kept == 0 { 0.0 } else { sum / kept as f32 })
}

#[cfg(test)]
mod tests {
    use super::*;

    const YML: &str = "Global:\n  model_name: x\nPostProcess:\n  name: CTCLabelDecode\n  character_dict:\n  - a\n  - 'b'\n  - '''\n  - \"\\u3042\"\n  - 中\n  - '#'\nmore: 1\n";

    #[test]
    fn the_dictionary_is_read_with_its_quoting() {
        let d = parse_dict(YML).unwrap();
        assert_eq!(d, ["a", "b", "'", "あ", "中", "#"]);
    }

    #[test]
    fn a_dictionary_that_is_not_one_is_refused() {
        assert!(parse_dict("nothing here").is_err());
        assert!(parse_dict("character_dict:\nnext: 1\n").is_err(), "an empty list");
        assert!(parse_dict("character_dict:\n  - 'unterminated\n").is_err());
        assert!(parse_dict("character_dict:\n  - \"\\uD800\"\n").is_err(), "a lone surrogate");
        assert!(parse_dict("character_dict:\n  - \"\\u12\"\n").is_err(), "a short escape");
        assert!(parse_dict(&format!("character_dict:\n  - {}\n", "x".repeat(100))).is_err(), "an entry too long");
    }

    /// `steps` rows with probability 0.9 on the given class.
    fn probs(classes: usize, picks: &[usize]) -> Vec<f32> {
        let mut v = vec![0.01; picks.len() * classes];
        for (t, &c) in picks.iter().enumerate() {
            v[t * classes + c] = 0.9;
        }
        v
    }

    #[test]
    fn ctc_merges_repeats_and_drops_blanks() {
        let dict: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        // classes: 0 blank, 1 a, 2 b, 3 c, 4 space
        let p = probs(5, &[1, 1, 0, 1, 2, 2, 0, 0, 3, 4, 1]);
        let (text, score) = decode(&p, 11, 5, &dict);
        assert_eq!(text, "aabc a");
        assert!((score - 0.9).abs() < 1e-6, "{score}");
        // all blank: no text, no score
        assert_eq!(decode(&probs(5, &[0, 0, 0]), 3, 5, &dict), (String::new(), 0.0));
    }

    #[test]
    fn ctc_survives_nonsense() {
        let dict: Vec<String> = vec!["a".into()];
        assert_eq!(decode(&[], 5, 3, &dict).0, "");
        assert!(decode(&[0.5; 4], 100, 3, &dict).0.len() <= 1, "fewer values than steps");
        assert_eq!(decode(&[0.5; 9], 3, 0, &dict), (String::new(), 0.0));
        // a class beyond the dictionary and the space is skipped, not a panic
        let p = probs(10, &[7, 8, 9]);
        assert_eq!(decode(&p, 3, 10, &dict).0, "");
        // NaN probabilities don't break it
        let _ = decode(&[f32::NAN; 6], 2, 3, &dict);
    }
}
