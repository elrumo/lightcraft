//! The search field. Plain words match a photo's text (file name, title, caption, camera, lens,
//! place fields, keywords, format); `field:value` tokens match one property (`rating:3`,
//! `iso:>800`, `camera:x2`, `date:2025`, `place:madrid`, `near:40.4,-3.7,5`, `bbox:s,w,n,e`).
//!
//! Beyond that the field understands a little natural language, so the way people write is the way
//! they search:
//!
//! - **Places.** "madrid", "photos in Madrid", "alcalá de henares", "españa", "new york" match the
//!   photos *taken* there — by their GPS position, through the offline gazetteer of `lightcraft-geo`
//!   — as well as photos whose text or IPTC city / state / country says so. A city covers its
//!   metropolitan area; a region or country covers everything whose nearest city is in it.
//! - **Dates.** "2024", "june", "june 2024" match by capture date (and still match text, so a file
//!   named `IMG_2024` is found too).
//! - **Fillers.** "photos", "pictures", "taken", "in", "at", "from", "near", "of", "the", "fotos",
//!   "de", "en"… are dropped when other words are present ("photos of madrid in june").
//! - **"or".** "madrid or paris" matches either; everything else is "and".
//!
//! A word that is also a place ("nice", "reading") still matches by text too, so natural language
//! only ever adds results to a plain-word search, never removes them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_geo::{Gazetteer, Kind, MAX_NAME_WORDS, PlaceFilter, PlaceId, normalize};

use crate::{Flag, Photo};

const FILLER: &[&str] = &[
    "a",
    "an",
    "the",
    "of",
    "in",
    "at",
    "from",
    "near",
    "nearby",
    "around",
    "by",
    "to",
    "on",
    "with",
    "and",
    "photos",
    "photo",
    "pictures",
    "picture",
    "pics",
    "pic",
    "images",
    "image",
    "shots",
    "shot",
    "taken",
    "show",
    "me",
    "my",
    "all",
    "during",
    "fotos",
    "foto",
    "imagenes",
    "imagen",
    "de",
    "del",
    "en",
    "la",
    "el",
    "los",
    "las",
    "cerca",
    "alrededor",
    "desde",
    "con",
    "y",
    "mis",
];

const MONTHS: [&str; 12] = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];

/// A compiled search-field text.
#[derive(Clone, Debug, Default)]
pub struct TextQuery {
    /// Alternatives joined by "or"; a photo matches when it satisfies every term of one.
    any: Vec<Vec<Term>>,
}

#[derive(Clone, Debug)]
enum Term {
    /// A `field:value` token, lower-cased.
    Field(String),
    Word(Word),
}

#[derive(Clone, Debug)]
struct Word {
    /// The word (or place name) as typed, lower-cased: matched as a substring of the photo's text.
    text: String,
    /// Match by text too (false for a month + year, which is only a date).
    by_text: bool,
    place: Option<Arc<PlaceFilter>>,
    date: Option<DateTerm>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DateTerm {
    Year(u16),
    Month(u8),
    MonthYear(u16, u8),
}

impl DateTerm {
    fn matches(self, date: &str) -> bool {
        let (y, m) = (date.get(..4), date.get(5..7));
        match self {
            DateTerm::Year(year) => y.is_some_and(|y| y == format!("{year:04}")),
            DateTerm::Month(month) => m.is_some_and(|m| m == format!("{month:02}")),
            DateTerm::MonthYear(year, month) => y.is_some_and(|y| y == format!("{year:04}")) && m.is_some_and(|m| m == format!("{month:02}")),
        }
    }
}

/// How a part of the search text was understood, for the interface to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Understood {
    /// `place`, `year`, `month` or `month-year`.
    pub kind: &'static str,
    /// "Madrid, Spain", "2024", "June".
    pub label: String,
    /// Photos of the library this part matches by itself (places only; 0 for dates).
    pub count: usize,
    /// Other places the same name could mean ("madrid" is also a town in Colombia).
    pub more: usize,
}

impl TextQuery {
    pub fn parse(text: &str) -> TextQuery {
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut any: Vec<Vec<Term>> = Vec::new();
        for group in words.split(|w| w.eq_ignore_ascii_case("or")) {
            let terms = parse_group(group);
            if !terms.is_empty() {
                any.push(terms);
            }
        }
        TextQuery { any }
    }

    /// Nothing to match: every photo passes.
    pub fn is_empty(&self) -> bool {
        self.any.is_empty()
    }

    pub fn matches(&self, p: &Photo) -> bool {
        self.any.is_empty() || self.any.iter().any(|terms| terms.iter().all(|t| t.matches(p)))
    }

    /// The places and dates the text was read as (not plain words or `field:` tokens). A place
    /// that no photo of `photos` is in is left out: "sunset" is also a town in Florida, but a
    /// library without photos there was not asked about it.
    pub fn understood(&self, photos: &[&Photo]) -> Vec<Understood> {
        let g = Gazetteer::global();
        let mut out = Vec::new();
        for t in self.any.iter().flatten() {
            let Term::Word(w) = t else { continue };
            if let Some(pl) = &w.place {
                let count = photos.iter().filter(|p| in_places(pl, p)).count();
                let best = pl.places().iter().max_by_key(|id| g.importance(**id)).copied();
                if let (Some(label), true) = (best.and_then(|id| g.label(id)), count > 0) {
                    let more = pl.places().len().saturating_sub(1);
                    out.push(Understood { kind: "place", label, count, more });
                }
            }
            match w.date {
                Some(DateTerm::Year(y)) => out.push(Understood { kind: "year", label: y.to_string(), count: 0, more: 0 }),
                Some(DateTerm::Month(m)) => out.push(Understood { kind: "month", label: month_name(m), count: 0, more: 0 }),
                Some(DateTerm::MonthYear(y, m)) => {
                    out.push(Understood { kind: "month-year", label: format!("{} {y}", month_name(m)), count: 0, more: 0 })
                }
                None => {}
            }
        }
        out
    }
}

fn month_name(m: u8) -> String {
    let n = MONTHS.get(usize::from(m).saturating_sub(1)).copied().unwrap_or("");
    let mut c = n.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn is_filler(w: &str) -> bool {
    FILLER.contains(&w.to_lowercase().as_str())
}

fn parse_year(w: &str) -> Option<u16> {
    (w.len() == 4 && w.bytes().all(|b| b.is_ascii_digit())).then(|| w.parse::<u16>().ok()).flatten().filter(|y| (1826..=2100).contains(y))
}

fn parse_month(w: &str) -> Option<u8> {
    let w = w.to_lowercase();
    MONTHS.iter().position(|m| *m == w).map(|i| i as u8 + 1)
}

fn parse_group(words: &[&str]) -> Vec<Term> {
    // "photos of madrid": drop fillers, but never all of them ("photos" alone searches for "photos")
    let content = words.iter().filter(|w| !w.contains(':') && !is_filler(w)).count() + words.iter().filter(|w| w.contains(':')).count();
    let drop_fillers = content > 0 && words.len() > 1;
    let mut terms = Vec::new();
    let mut i = 0;
    while let Some(&w) = words.get(i) {
        if let Some((field, value)) = w.split_once(':') {
            terms.push(Term::Field(w.to_lowercase()));
            let _ = (field, value);
            i += 1;
            continue;
        }
        if let Some((n, text, pl)) = place_at(words, i) {
            terms.push(Term::Word(Word { text, by_text: true, place: Some(pl), date: None }));
            i += n;
            continue;
        }
        if let Some(month) = parse_month(w) {
            if let Some(year) = words.get(i + 1).and_then(|n| parse_year(n)) {
                terms.push(Term::Word(Word {
                    text: format!("{} {year}", w.to_lowercase()),
                    by_text: false,
                    place: None,
                    date: Some(DateTerm::MonthYear(year, month)),
                }));
                i += 2;
            } else {
                terms.push(Term::Word(Word { text: w.to_lowercase(), by_text: true, place: None, date: Some(DateTerm::Month(month)) }));
                i += 1;
            }
            continue;
        }
        if let Some(year) = parse_year(w) {
            terms.push(Term::Word(Word { text: w.to_string(), by_text: true, place: None, date: Some(DateTerm::Year(year)) }));
            i += 1;
            continue;
        }
        if !(drop_fillers && is_filler(w)) {
            terms.push(Term::Word(Word { text: w.to_lowercase(), by_text: true, place: None, date: None }));
        }
        i += 1;
    }
    terms
}

/// The longest run of words starting at `i` that names a place: (words used, the words, the places).
fn place_at(words: &[&str], i: usize) -> Option<(usize, String, Arc<PlaceFilter>)> {
    let g = Gazetteer::global();
    if g.is_empty() {
        return None;
    }
    for n in (1..=MAX_NAME_WORDS.min(words.len().saturating_sub(i))).rev() {
        let run = words.get(i..i + n)?;
        if run.iter().any(|w| w.contains(':')) {
            continue;
        }
        let phrase = run.join(" ");
        // a short plain word ("la", "in", "or") is a word, not a place
        if n == 1 && phrase.chars().count() < if phrase.is_ascii() { 3 } else { 2 } {
            continue;
        }
        if let Some(pl) = place_filter(&phrase) {
            return Some((n, phrase.to_lowercase(), pl));
        }
    }
    None
}

/// The compiled places a name stands for (`None` when it names none). Cached: smart-album rules
/// evaluate the same name for every photo.
pub fn place_filter(name: &str) -> Option<Arc<PlaceFilter>> {
    place_filter_of(name, |_| true)
}

fn place_filter_of(name: &str, keep: impl Fn(Kind) -> bool) -> Option<Arc<PlaceFilter>> {
    type Cache = Mutex<HashMap<(String, bool, bool, bool), Option<Arc<PlaceFilter>>>>;
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    let key = (normalize(name), keep(Kind::City), keep(Kind::Region), keep(Kind::Country));
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
        return hit.clone();
    }
    let g = Gazetteer::global();
    let ids: Vec<PlaceId> = g.lookup(&key.0).into_iter().filter(|i| keep(i.kind)).collect();
    let made = (!ids.is_empty()).then(|| Arc::new(PlaceFilter::new(g, &ids)));
    let mut c = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if c.len() >= 256 {
        c.clear();
    }
    c.insert(key, made.clone());
    made
}

/// Whether a photo was taken in (or is labelled as being in) the place `name` stands for.
pub fn photo_in_place(p: &Photo, name: &str) -> bool {
    place_filter(name).is_some_and(|pl| in_places(&pl, p))
}

/// Whether the photo has any idea where it was taken: GPS or place fields.
pub fn has_place(p: &Photo) -> bool {
    let m = &p.meta;
    m.gps.is_some() || [&m.location, &m.city, &m.state, &m.country].iter().any(|s| !s.trim().is_empty())
}

fn in_places(pl: &PlaceFilter, p: &Photo) -> bool {
    let g = Gazetteer::global();
    let m = &p.meta;
    if let Some((lat, lon)) = m.gps
        && pl.contains(g, lat, lon)
    {
        return true;
    }
    [&m.city, &m.state, &m.country, &m.location].iter().any(|s| pl.names(g, s))
}

impl Term {
    fn matches(&self, p: &Photo) -> bool {
        match self {
            Term::Field(tok) => field_matches(p, tok),
            Term::Word(w) => w.matches(p),
        }
    }
}

impl Word {
    fn matches(&self, p: &Photo) -> bool {
        (self.by_text && text_contains(p, &self.text))
            || self.date.is_some_and(|d| d.matches(p.date()))
            || self.place.as_deref().is_some_and(|pl| in_places(pl, p))
    }
}

fn text_contains(p: &Photo, t: &str) -> bool {
    let m = &p.meta;
    let hay = [&p.file_name, &m.title, &m.caption, &m.camera, &m.lens, &m.location, &m.city, &m.state, &m.country, &p.format];
    hay.iter().any(|h| contains_ci(h, t)) || m.keywords.iter().any(|k| contains_ci(k, t))
}

/// Whether `hay` contains `needle` (already lower-cased), ignoring case. ASCII text, nearly all of
/// it, is compared in place; searching a big library on every keystroke shouldn't allocate.
fn contains_ci(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if hay.is_ascii() && needle.is_ascii() {
        let n = needle.as_bytes();
        return hay.len() >= n.len() && hay.as_bytes().windows(n.len()).any(|w| w.eq_ignore_ascii_case(n));
    }
    hay.to_lowercase().contains(needle)
}

pub(crate) fn has_person(p: &Photo, name: &str) -> bool {
    let name = name.trim().to_lowercase();
    p.meta.regions.iter().any(|r| r.kind == lightcraft_meta::RegionKind::Face && r.name.as_deref().is_some_and(|n| n.to_lowercase() == name))
}

/// `field:value` (already lower-cased).
fn field_matches(p: &Photo, t: &str) -> bool {
    let Some((field, val)) = t.split_once(':') else { return false };
    let num = |s: &str| -> Option<(char, f64)> {
        let (op, rest) = match s.chars().next()? {
            c @ ('>' | '<' | '=') => (c, &s[1..]),
            _ => ('=', s),
        };
        rest.parse().ok().map(|v| (op, v))
    };
    let cmp = |x: f64, s: &str| match num(s) {
        Some(('>', v)) => x > v,
        Some(('<', v)) => x < v,
        Some((_, v)) => (x - v).abs() < 1e-9,
        None => false,
    };
    let yes = |v: &str| matches!(v, "true" | "yes" | "y" | "1");
    match field {
        "rating" | "stars" => cmp(p.rating as f64, val),
        "flag" => Flag::parse(val) == Some(p.flag),
        "label" | "color" => p.label.is_some_and(|l| format!("{l:?}").eq_ignore_ascii_case(val)),
        "iso" => p.meta.iso.is_some_and(|i| cmp(i as f64, val)),
        "f" | "aperture" => p.meta.aperture.is_some_and(|a| cmp(a as f64, val)),
        "focal" => p.meta.focal_mm.is_some_and(|a| cmp(a as f64, val)),
        "camera" => p.meta.camera.to_lowercase().contains(val),
        "lens" => p.meta.lens.to_lowercase().contains(val),
        "keyword" | "kw" => p.meta.keywords.iter().any(|k| crate::keywords::is_under(k, val)),
        "person" | "who" => has_person(p, val),
        "type" | "kind" => format!("{:?}", p.kind).eq_ignore_ascii_case(val),
        "edited" => yes(val) == p.is_edited(),
        "date" => p.date().starts_with(val),
        "copy" | "virtual" => yes(val) == p.copy_of.is_some(),
        "name" | "file" => p.file_name.to_lowercase().contains(val),
        "gps" | "geotagged" | "located" => yes(val) == p.meta.gps.is_some(),
        "place" | "where" | "in" | "loc" | "location" => place_field(p, val, |_| true),
        "city" => place_field(p, val, |k| k == Kind::City),
        "region" | "state" | "province" => place_field(p, val, |k| k == Kind::Region),
        "country" => place_field(p, val, |k| k == Kind::Country),
        "near" => near(p, val),
        "bbox" => in_bbox(p, val),
        _ => false,
    }
}

fn place_field(p: &Photo, val: &str, keep: impl Fn(Kind) -> bool) -> bool {
    let name = val.replace(['_', '+'], " ");
    if Gazetteer::global().lookup_text(&name).is_empty() {
        // not a place we know: the text of the photo's own place fields
        let name = name.to_lowercase();
        let m = &p.meta;
        return [&m.location, &m.city, &m.state, &m.country].iter().any(|s| s.to_lowercase().contains(&name));
    }
    place_filter_of(&name, keep).is_some_and(|pl| in_places(&pl, p))
}

fn floats(s: &str) -> Option<Vec<f64>> {
    s.split(',').map(|x| x.trim().parse::<f64>().ok().filter(|v| v.is_finite())).collect()
}

/// `near:lat,lon[,km]` (default 1 km).
fn near(p: &Photo, val: &str) -> bool {
    let (Some(v), Some((lat, lon))) = (floats(val), p.meta.gps) else { return false };
    match v.as_slice() {
        [la, lo] => lightcraft_geo::geodesy::distance_km(lat, lon, *la, *lo) <= 1.0,
        [la, lo, km] => lightcraft_geo::geodesy::distance_km(lat, lon, *la, *lo) <= *km,
        _ => false,
    }
}

/// `bbox:south,west,north,east` (west > east crosses the antimeridian).
fn in_bbox(p: &Photo, val: &str) -> bool {
    let (Some(v), Some((lat, lon))) = (floats(val), p.meta.gps) else { return false };
    let [s, w, n, e] = v.as_slice() else { return false };
    let lon_ok = if w <= e { (*w..=*e).contains(&lon) } else { lon >= *w || lon <= *e };
    (*s..=*n).contains(&lat) && lon_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Catalog;

    /// Photos taken in/near Madrid, Barcelona, Paris and Sydney, plus one with no GPS.
    fn library() -> (Catalog, [crate::PhotoId; 6]) {
        let mut c = Catalog::default();
        let mut add = |name: &str, date: &str, gps: Option<(f64, f64)>, f: &dyn Fn(&mut crate::Meta)| {
            let id = crate::tests::photo(&mut c, name, date);
            set(&mut c, id, |m| {
                m.gps = gps;
                f(m)
            });
            id
        };
        let sol = add("sol.jpg", "2024-06-10T12:00:00", Some((40.4169, -3.7035)), &|_| {});
        let alcobendas = add("alco.jpg", "2023-03-02T12:00:00", Some((40.54, -3.64)), &|_| {});
        let bcn = add("bcn.jpg", "2024-06-20T12:00:00", Some((41.3874, 2.1686)), &|_| {});
        let paris = add("eiffel.jpg", "2022-08-01T12:00:00", Some((48.8584, 2.2945)), &|_| {});
        let sydney = add("opera.jpg", "2024-01-05T12:00:00", Some((-33.8568, 151.2153)), &|_| {});
        let tagged = add("iptc.jpg", "2021-05-05T12:00:00", None, &|m| {
            m.city = "Madrid".into();
            m.country = "Spain".into();
        });
        (c, [sol, alcobendas, bcn, paris, sydney, tagged])
    }

    fn set(c: &mut Catalog, id: crate::PhotoId, f: impl FnOnce(&mut crate::Meta)) {
        let mut m = c.photo(id).unwrap().meta.clone();
        f(&mut m);
        c.apply(crate::Op::SetMeta { id, meta: Box::new(m) }).unwrap();
    }

    fn q(c: &Catalog, text: &str) -> Vec<crate::PhotoId> {
        let mut v = c.query(&crate::Filter { text: text.into(), ..Default::default() }, &crate::Sort::default());
        v.sort();
        v
    }

    fn ids(v: &[crate::PhotoId]) -> Vec<crate::PhotoId> {
        let mut v = v.to_vec();
        v.sort();
        v
    }

    #[test]
    fn a_city_name_finds_what_was_shot_there() {
        let (c, [sol, alco, _bcn, _paris, _syd, tagged]) = library();
        assert_eq!(q(&c, "madrid"), ids(&[sol, alco, tagged]), "centre, suburb, and the photo that only says so in its IPTC fields");
        assert_eq!(q(&c, "MADRID"), q(&c, "madrid"));
        assert_eq!(q(&c, "sydnye"), vec![], "misspellings are not guessed");
        assert_eq!(q(&c, "sidney").len(), 1, "but alternate spellings GeoNames lists are known");
    }

    #[test]
    fn natural_phrasing() {
        let (c, [sol, alco, bcn, paris, syd, tagged]) = library();
        assert_eq!(q(&c, "photos in madrid"), q(&c, "madrid"));
        assert_eq!(q(&c, "pictures taken in Madrid"), q(&c, "madrid"));
        assert_eq!(q(&c, "fotos en madrid"), q(&c, "madrid"));
        assert_eq!(q(&c, "madrid 2024"), vec![sol]);
        assert_eq!(q(&c, "photos of barcelona in june 2024"), vec![bcn]);
        assert_eq!(q(&c, "madrid or paris"), ids(&[sol, alco, tagged, paris]));
        assert_eq!(q(&c, "sydney"), vec![syd]);
        assert_eq!(q(&c, "españa"), ids(&[sol, alco, bcn, tagged]), "a country by another name");
        assert_eq!(q(&c, "spain"), q(&c, "españa"));
        assert_eq!(q(&c, "madrid spain"), ids(&[sol, alco, tagged]), "two places are both required");
        assert_eq!(q(&c, "madrid france"), vec![]);
    }

    #[test]
    fn dates_in_words() {
        let (c, [sol, _alco, bcn, _paris, syd, _tagged]) = library();
        assert_eq!(q(&c, "2024"), ids(&[sol, bcn, syd]));
        assert_eq!(q(&c, "june"), ids(&[sol, bcn]));
        assert_eq!(q(&c, "june 2024"), ids(&[sol, bcn]));
        assert_eq!(q(&c, "january 2023"), vec![]);
        assert_eq!(q(&c, "sydney 2024"), vec![syd]);
    }

    #[test]
    fn plain_words_still_work_and_fillers_are_not_swallowed() {
        let (mut c, [sol, ..]) = library();
        set(&mut c, sol, |m| m.title = "photos of the sunset".into());
        assert_eq!(q(&c, "sunset"), vec![sol]);
        assert_eq!(q(&c, "photos"), vec![sol], "alone, a filler word is just a word");
        assert_eq!(q(&c, "photos of the"), vec![sol]);
        assert_eq!(q(&c, ""), q(&c, "   "));
        assert_eq!(q(&c, "").len(), 6);
    }

    #[test]
    fn fielded_tokens_keep_working() {
        let (c, [sol, ..]) = library();
        assert_eq!(q(&c, "date:2024-06-10"), vec![sol]);
        assert_eq!(q(&c, "nosuchfield:1"), vec![]);
        assert_eq!(q(&c, "gps:no").len(), 1);
        assert_eq!(q(&c, "gps:yes").len(), 5);
    }

    #[test]
    fn place_tokens() {
        let (c, [sol, alco, bcn, _p, _s, tagged]) = library();
        assert_eq!(q(&c, "place:madrid"), ids(&[sol, alco, tagged]));
        assert_eq!(q(&c, "country:spain"), ids(&[sol, alco, bcn, tagged]));
        assert_eq!(q(&c, "city:spain"), vec![], "spain is not a city");
        assert_eq!(q(&c, "place:new_york"), vec![]);
        assert_eq!(q(&c, "near:40.4169,-3.7035"), vec![sol]);
        assert_eq!(q(&c, "near:40.4169,-3.7035,20"), ids(&[sol, alco]));
        assert_eq!(q(&c, "bbox:40,-4,41,-3"), ids(&[sol, alco]));
        assert_eq!(q(&c, "bbox:40,-4,41"), vec![], "four numbers or nothing");
        assert_eq!(q(&c, "near:abc"), vec![]);
    }

    #[test]
    fn a_word_that_is_also_a_place_still_matches_text() {
        let (mut c, [sol, _a, bcn, ..]) = library();
        set(&mut c, sol, |m| m.title = "A nice evening".into());
        let got = q(&c, "nice");
        assert!(got.contains(&sol), "text still matches: {got:?}");
        assert!(!got.contains(&bcn));
    }

    #[test]
    fn what_was_understood() {
        let (c, _) = library();
        let all: Vec<&Photo> = c.photos().map(|p| p.as_ref()).collect();
        let u = |t: &str| TextQuery::parse(t).understood(&all);
        let m = u("madrid");
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].kind, m[0].count), ("place", 3));
        assert!(m[0].label.starts_with("Madrid, Spain"), "{}", m[0].label);
        assert_eq!(u("june 2024"), vec![Understood { kind: "month-year", label: "June 2024".into(), count: 0, more: 0 }]);
        assert_eq!(u("2024")[0].kind, "year");
        assert!(u("sunset").is_empty(), "a town called Sunset, but no photo there: not worth a chip");
        assert!(u("rating:3").is_empty());
        assert!(u("tokyo").is_empty(), "a known place with no photos");
    }

    #[test]
    fn case_blind_containment() {
        assert!(contains_ci("IMG_0042.CR2", "img_00") && contains_ci("IMG_0042.CR2", "cr2") && contains_ci("abc", ""));
        assert!(!contains_ci("abc", "abcd") && !contains_ci("", "a"));
        assert!(
            contains_ci("Alcalá de Henares", "alca") && contains_ci("Alcalá de Henares", "alcalá") && !contains_ci("Alcalá de Henares", "alcala")
        );
        assert!(contains_ci("ÉCOLE", "école") && contains_ci("東京タワー", "東京"));
    }

    #[test]
    fn hostile_text_is_just_text() {
        let (c, _) = library();
        for t in [
            "::::",
            ":",
            "a:b:c",
            "near:,,,",
            "bbox:nan,nan,nan,nan",
            "bbox:inf,0,0,0",
            "or",
            "or or or",
            "\u{0}\u{1}",
            "ÀÉÎ",
            "🙂",
            &"x ".repeat(2000),
            &"madrid ".repeat(500),
        ] {
            let _ = q(&c, t);
        }
    }

    #[test]
    fn the_place_cache_is_bounded() {
        for i in 0..600 {
            let _ = place_filter(&format!("nowhere {i}"));
        }
        assert!(place_filter("madrid").is_some());
    }
}

#[cfg(test)]
mod perf {
    use super::*;
    use crate::{Catalog, Filter, Sort};

    /// `cargo test -p lightcraft-catalog --release -- --ignored --nocapture search_speed`
    #[test]
    #[ignore = "timing, not correctness"]
    fn search_speed() {
        let mut c = Catalog::default();
        let mut seed = 12345u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as f64 / (1u64 << 31) as f64
        };
        for i in 0..85_000 {
            let id = crate::tests::photo(&mut c, &format!("IMG_{i}.jpg"), "2024-06-10T12:00:00");
            let mut m = c.photo(id).unwrap().meta.clone();
            m.gps = (i % 4 != 0).then(|| (-60.0 + next() * 120.0, -180.0 + next() * 360.0));
            c.apply(crate::Op::SetMeta { id, meta: Box::new(m) }).unwrap();
        }
        let t = std::time::Instant::now();
        let _ = Gazetteer::global();
        println!("gazetteer load: {:?}", t.elapsed());
        // best of seven: the machine may be busy
        let best = |f: &mut dyn FnMut() -> usize| {
            (0..7)
                .map(|_| {
                    let t = std::time::Instant::now();
                    let n = f();
                    (t.elapsed(), n)
                })
                .min()
                .unwrap()
        };
        for text in ["", "IMG_1", "madrid", "spain", "photos in tokyo or paris in june 2024", "country:france", "near:40.4,-3.7,50"] {
            let (d, n) = best(&mut || c.query(&Filter { text: text.into(), ..Default::default() }, &Sort::default()).len());
            println!("{text:>42?}: {n:>6} photos in {d:?}");
        }
        let photos: Vec<&Photo> = c.photos().map(|p| p.as_ref()).collect();
        let pl = place_filter("madrid").unwrap();
        let (d, n) = best(&mut || photos.iter().filter(|p| in_places(&pl, p)).count());
        println!("   in_places only: {n} in {d:?}");
        let (d, n) = best(&mut || photos.iter().filter(|p| text_contains(p, "madrid")).count());
        println!("   text_contains only: {n} in {d:?}");
        let g = Gazetteer::global();
        let (d, n) = best(&mut || photos.iter().filter(|p| p.meta.gps.is_some_and(|(a, b)| pl.contains(g, a, b))).count());
        println!("   PlaceFilter::contains only: {n} in {d:?}");
        let (d, n) = best(&mut || photos.iter().filter(|p| p.meta.gps.is_some_and(|(a, b)| g.locate_cell(a, b).is_some())).count());
        println!("   locate_cell only: {n} in {d:?}");
        let t = std::time::Instant::now();
        let u = c.understood(&Filter { text: "madrid".into(), ..Default::default() });
        println!("understood: {u:?} in {:?}", t.elapsed());
    }
}
