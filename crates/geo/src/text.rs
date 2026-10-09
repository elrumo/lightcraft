//! Place-name normalisation: what "Alcalá de Henares", "alcala de henares" and "ALCALA-DE-HENARES"
//! have in common.

/// Lower-case, fold Latin diacritics (`é` → `e`, `ß` → `ss`, `ø` → `o`), and make every run of
/// punctuation or whitespace a single space. Letters of other scripts (CJK, Cyrillic, Arabic…)
/// are kept as they are, lower-cased.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    for c in s.chars() {
        for lc in c.to_lowercase() {
            if ('\u{300}'..='\u{36f}').contains(&lc) {
                continue; // combining accents (NFD input, or `İ` lower-cased)
            }
            if !lc.is_alphanumeric() {
                gap = true;
                continue;
            }
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            match fold(lc) {
                Some(f) => out.push_str(f),
                None => out.push(lc),
            }
        }
    }
    out
}

/// The ASCII spelling of a lower-case Latin letter with a diacritic, when it has one.
fn fold(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'ǎ' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' | 'ǐ' => "i",
        'ĵ' => "j",
        'ķ' | 'ĸ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' | 'ŋ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' | 'ǒ' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' | 'ț' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' | 'ǔ' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_accents_case_and_punctuation() {
        assert_eq!(normalize("Alcalá de Henares"), "alcala de henares");
        assert_eq!(normalize("  ALCALA-DE--HENARES! "), "alcala de henares");
        assert_eq!(normalize("Köln"), "koln");
        assert_eq!(normalize("Straße"), "strasse");
        assert_eq!(normalize("Łódź"), "lodz");
        assert_eq!(normalize("Reykjavík"), "reykjavik");
        assert_eq!(normalize("St. John's"), "st john s");
    }

    #[test]
    fn keeps_other_scripts() {
        assert_eq!(normalize("東京"), "東京");
        assert_eq!(normalize("Москва"), "москва");
        assert_eq!(normalize("서울"), "서울");
    }

    #[test]
    fn decomposed_accents_are_dropped() {
        assert_eq!(normalize("Cafe\u{301}"), "cafe");
        assert_eq!(normalize("İstanbul"), "istanbul");
    }

    #[test]
    fn empty_and_symbols() {
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("---"), "");
        assert_eq!(normalize("a  b"), "a b");
    }
}
