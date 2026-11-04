//! The compact form of the develop settings in `catalog.snap` (catalog format v13).
//!
//! A photo's develop settings are about 3 KB of JSON, and most photos of a library are never edited, so most of a big
//! snapshot was the same defaults written again for every photo (an 845 MB snapshot of 228,000 photos was two thirds
//! settings, nine photos in ten of them the same; in this form it is 262 MB). A v13 snapshot names the develop defaults
//! of the build that wrote
//! it once, in its header (`"defaults":{…}` before `"catalog":`), and each settings object of a photo (its edit,
//! import look, versions and History steps) holds only what differs from them, as the sync server's compact pages do
//! ([`crate::sync::diff`]): an untouched photo's settings are `{}`. Reading patches each one back onto the file's own
//! defaults, so a later build whose defaults changed reads every photo as it was written, and every untouched photo
//! of the file shares one copy of its settings in memory.
//!
//! A diff gives back the very JSON it came from (numbers compared as written: `-0.0` stays `-0.0`), except where that
//! JSON has a `null` member, which a merge patch can only say as "take it out": settings with one of those are
//! written whole, as `[{…}]` ([`lossless`]). Only the snapshot is written this way: the
//! log's records, the sync protocol and every other JSON of a photo carry its settings whole, as before.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use serde::de::{self, MapAccess};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use crate::sync::{diff, patch, same};

/// The defaults a snapshot is written against.
struct Writer {
    /// The defaults, and as JSON.
    typed: DevelopSettings,
    value: Value,
    /// `value`'s text, to know an untouched photo's settings without building their tree (most photos).
    text: Vec<u8>,
    /// What was written for a settings object that several photos or steps share, by its address (after a v13 read,
    /// every untouched photo shares one): worked out once. The catalog being written holds them, so no address is
    /// reused meanwhile; settings held once are not kept here.
    shared: HashMap<usize, Value>,
}

/// The defaults a snapshot was written against, for reading it.
pub(crate) struct Reader {
    value: Value,
    /// The settings of an untouched photo, shared by all of them.
    settings: Arc<DevelopSettings>,
}

impl Reader {
    /// The reader of a snapshot whose header named `defaults`.
    pub(crate) fn new(defaults: Value) -> serde_json::Result<Reader> {
        // (an object, as the writer writes them: serde would also take settings as a list of their members' values)
        if !defaults.is_object() {
            return Err(serde::de::Error::custom("the develop defaults are not an object"));
        }
        let settings = Arc::new(DevelopSettings::deserialize(&defaults)?);
        Ok(Reader { value: defaults, settings })
    }
}

thread_local! {
    static WRITING: RefCell<Option<Writer>> = const { RefCell::new(None) };
    static READING: RefCell<Option<Arc<Reader>>> = const { RefCell::new(None) };
}

/// While it lives, the photos serialized on this thread write their settings compactly ([`Writing::defaults`] is
/// what the header must name). `None` defaults (the defaults don't read back as themselves, which would make every
/// diff wrong): the snapshot is written whole, with no `defaults` in its header.
pub(crate) struct Writing(Option<Value>);

impl Writing {
    pub(crate) fn enter() -> Writing {
        let defaults = DevelopSettings::default();
        let value = serde_json::to_value(&defaults)
            .ok()
            .filter(|v| DevelopSettings::deserialize(v).and_then(serde_json::to_value).is_ok_and(|back| same(&back, v)));
        if let (Some(value), Ok(text)) = (&value, serde_json::to_vec(&defaults)) {
            WRITING.with(|w| *w.borrow_mut() = Some(Writer { typed: defaults, value: value.clone(), text, shared: HashMap::new() }));
        }
        Writing(value)
    }

    pub(crate) fn defaults(&self) -> Option<&Value> {
        self.0.as_ref()
    }
}

impl Drop for Writing {
    fn drop(&mut self) {
        WRITING.with(|w| *w.borrow_mut() = None);
    }
}

/// While it lives, the settings read on this thread are diffs against `reader`'s defaults (none: whole settings).
pub(crate) struct Reading;

impl Reading {
    pub(crate) fn enter(reader: Option<Arc<Reader>>) -> Reading {
        READING.with(|r| *r.borrow_mut() = reader);
        Reading
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        READING.with(|r| *r.borrow_mut() = None);
    }
}

/// `#[serde(serialize_with)]` for a photo's settings: whole, or (while [`Writing`]) what differs from the defaults.
pub(crate) fn develop_out<S: Serializer>(settings: &Arc<DevelopSettings>, ser: S) -> Result<S::Ok, S::Error> {
    WRITING.with(|w| {
        let mut w = w.borrow_mut();
        let Some(w) = w.as_mut() else { return settings.serialize(ser) };
        if Arc::strong_count(settings) < 2 {
            return compact_form(w, settings).map_err(serde::ser::Error::custom)?.serialize(ser);
        }
        let key = Arc::as_ptr(settings) as usize;
        if !w.shared.contains_key(&key) {
            let made = compact_form(w, settings).map_err(serde::ser::Error::custom)?;
            w.shared.insert(key, made);
        }
        match w.shared.get(&key) {
            Some(v) => v.serialize(ser),
            None => settings.serialize(ser),
        }
    })
}

/// What a v13 snapshot writes for `settings`: what differs from the defaults (`{}` for nothing), or `[{…}]` when that
/// would not read back as the same JSON.
fn compact_form(w: &Writer, settings: &DevelopSettings) -> serde_json::Result<Value> {
    // (equal ones are the same JSON when their text is too: down to the sign of a zero)
    if *settings == w.typed && serde_json::to_vec(settings)? == w.text {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let whole = serde_json::to_value(settings)?;
    Ok(if lossless(&w.value, &whole) { diff(&w.value, &whole) } else { Value::Array(vec![whole]) })
}

/// [`patch`] gives `v` back from [`diff`]`(base, v)`: no member that goes into the diff is `null` (walked as `diff`
/// walks them; a `null` there would read back as a member taken out).
fn lossless(base: &Value, v: &Value) -> bool {
    let (Value::Object(b), Value::Object(o)) = (base, v) else { return true };
    o.iter().all(|(k, ov)| match b.get(k) {
        Some(bv) if same(bv, ov) => true,
        Some(bv @ Value::Object(_)) if ov.is_object() => lossless(bv, ov),
        _ => !ov.is_null(),
    })
}

/// [`develop_out`] for an optional settings object (`None` is skipped by the field's `skip_serializing_if`).
pub(crate) fn develop_opt_out<S: Serializer>(settings: &Option<Arc<DevelopSettings>>, ser: S) -> Result<S::Ok, S::Error> {
    match settings {
        Some(s) => develop_out(s, ser),
        None => ser.serialize_none(),
    }
}

/// Settings written as a JSON object: whole, or (while [`Reading`]) a diff to patch onto the file's defaults.
pub(crate) fn from_map<'de, A: MapAccess<'de>>(map: A) -> Result<Arc<DevelopSettings>, A::Error> {
    let Some(reader) = READING.with(|r| r.borrow().clone()) else {
        return DevelopSettings::deserialize(de::value::MapAccessDeserializer::new(map)).map(Arc::new);
    };
    let d = Value::deserialize(de::value::MapAccessDeserializer::new(map))?;
    if d.as_object().is_some_and(|m| m.is_empty()) {
        return Ok(reader.settings.clone());
    }
    DevelopSettings::deserialize(&patch(&reader.value, &d)).map(Arc::new).map_err(de::Error::custom)
}

/// Settings written whole inside a list (`[{…}]`, what [`develop_out`] writes for a diff that would not read back).
pub(crate) fn from_whole(v: Value) -> serde_json::Result<Arc<DevelopSettings>> {
    DevelopSettings::deserialize(&v).map(Arc::new)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lightcraft_develop::DevelopSettings;

    use super::{diff, lossless, patch, same};
    use crate::journal::{Journal, SNAPSHOT};
    use crate::model::{HistoryStep, Version};
    use crate::*;

    /// `untouched` photos with the default settings, then `edited` ones whose edit, import look, version and History
    /// differ from them.
    fn library(untouched: u64, edited: u64) -> Catalog {
        let mut c = Catalog::new();
        let ops = (0..untouched + edited)
            .map(|i| {
                let mut p = Photo::new(PhotoId(i + 1), Source::Demo { scene: 0 }, &format!("IMG_{i}.jpg"), "JPEG", 600, 400, "2026-01-01T00:00:00");
                if i >= untouched {
                    let mut d = DevelopSettings::default();
                    d.light.exposure = 0.25 * (i % 5 + 1) as f64;
                    d.crop.aspect = Some((3, 2));
                    let d = Arc::new(d);
                    p.develop = d.clone();
                    p.import_look = Some(d.clone());
                    p.versions = vec![Version { name: "Mine".into(), created: "2026-01-02T00:00:00".into(), settings: d.clone(), auto: false }];
                    p.history = vec![
                        HistoryStep { label: "Import".into(), settings: Arc::new(DevelopSettings::default()) },
                        HistoryStep { label: "Exposure".into(), settings: d },
                    ];
                }
                Op::AddPhoto { photo: Box::new(p) }
            })
            .collect();
        c.apply(Op::Batch { ops }).unwrap();
        c
    }

    /// `c` saved as a library's snapshot, and the snapshot's text.
    fn saved(c: &Catalog) -> (MemStore, String) {
        let m = MemStore::new();
        let (mut j, ..) = Journal::open(Box::new(m.clone())).unwrap();
        j.snapshot(c).unwrap();
        drop(j);
        let text = String::from_utf8(m.get(SNAPSHOT).unwrap()).unwrap();
        (m, text)
    }

    /// The snapshot `text` with other `defaults` in its header.
    fn with_defaults(text: &str, defaults: &str) -> Vec<u8> {
        let catalog = &text[text.find("\"catalog\":").unwrap()..];
        format!("{{\"format\":\"lightcraft-catalog\",\"version\":{},\"seq\":0,\"defaults\":{defaults},{catalog}", crate::journal::VERSION)
            .into_bytes()
    }

    #[test]
    fn a_compact_snapshot_reopens_as_the_same_library_and_untouched_photos_share_their_settings() {
        let c = library(300, 30);
        let (m, text) = saved(&c);
        assert!(text.starts_with(&format!("{{\"format\":\"lightcraft-catalog\",\"version\":{},\"seq\":0,\"defaults\":{{", crate::journal::VERSION)));
        assert!(text.contains("\"develop\":{},\"edited\":"), "an untouched photo's settings are empty");
        let (_, opened, report) = Journal::open(Box::new(m)).unwrap();
        assert_eq!(report.upgraded_from, None);
        assert!(opened.to_snapshot() == c.to_snapshot(), "the reopened library is not the saved one");
        let first = opened.photo(PhotoId(1)).unwrap().develop.clone();
        assert!(opened.photos().take(300).all(|p| Arc::ptr_eq(&p.develop, &first)), "untouched photos share one copy");
    }

    #[test]
    fn a_zero_keeps_its_sign_in_the_snapshot_and_on_the_wire() {
        let mut d = DevelopSettings::default();
        d.light.contrast = -0.0;
        let mut c = Catalog::new();
        let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, "IMG.jpg", "JPEG", 600, 400, "2026-01-01T00:00:00");
        p.develop = Arc::new(d);
        c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        let (m, text) = saved(&c);
        assert!(text.contains("\"contrast\":-0.0"), "written, not taken for the default 0.0");
        let (_, opened, _) = Journal::open(Box::new(m)).unwrap();
        assert!(opened.photo(PhotoId(1)).unwrap().develop.light.contrast.is_sign_negative());
        assert!(opened.to_snapshot() == c.to_snapshot());
        // the server's compact pages diff the same way
        let page = crate::sync::proto::Snapshot { defaults: None, library: "l".into(), seq: 1, catalog: c.clone(), next: None, total: None };
        let wire = serde_json::to_string(&crate::sync::compact_page(page).unwrap()).unwrap();
        let back = crate::sync::proto::Snapshot::from_json(&wire).unwrap();
        assert!(back.catalog.to_snapshot() == c.to_snapshot());
    }

    #[test]
    fn a_diff_with_a_null_member_is_not_lossless() {
        let base = serde_json::json!({"a": 1, "o": {"x": 1, "y": null}});
        assert!(lossless(&base, &serde_json::json!({"a": 2, "o": {"x": 1, "y": null}})), "a null the defaults have too stays out of the diff");
        assert!(lossless(&base, &serde_json::json!({"a": 1})), "a member taken out is a null in the diff, read back as taken out");
        assert!(!lossless(&base, &serde_json::json!({"a": null, "o": {"x": 1, "y": null}})), "null where the defaults have 1");
        assert!(!lossless(&base, &serde_json::json!({"a": 1, "o": {"x": null}})), "a null inside a member");
        assert!(!lossless(&base, &serde_json::json!({"a": 1, "new": null})), "a null the defaults have no member for");
        for v in [serde_json::json!({"a": 2, "o": {"x": 1, "y": 3}}), serde_json::json!({"o": [1, null]}), serde_json::json!({"a": -0.0})] {
            assert!(lossless(&base, &v) && same(&patch(&base, &diff(&base, &v)), &v), "{v}");
        }
    }

    #[test]
    fn it_is_a_fraction_of_the_whole_form() {
        let c = library(1000, 0);
        let (_, text) = saved(&c);
        let whole = c.to_snapshot().len();
        assert!(text.len() * 3 < whole, "{} bytes compact, {whole} whole", text.len());
    }

    #[test]
    fn photos_keep_the_settings_of_the_defaults_they_were_written_with() {
        let c = library(5, 5);
        let (m, text) = saved(&c);
        // the file was written by a build whose defaults differ from this one's: its own still decide
        let mut then = DevelopSettings::default();
        then.light.exposure = 0.5;
        then.color.vibrance = 7.0;
        m.set(SNAPSHOT, with_defaults(&text, &serde_json::to_string(&then).unwrap()));
        let (_, opened, _) = Journal::open(Box::new(m)).unwrap();
        assert_eq!(*opened.photo(PhotoId(1)).unwrap().develop, then, "an untouched photo has the defaults it was written with");
        let edited = &opened.photo(PhotoId(6)).unwrap().develop;
        assert_eq!(edited.light.exposure, c.photo(PhotoId(6)).unwrap().develop.light.exposure, "what the edit changed");
        assert_eq!((edited.crop.aspect, edited.color.vibrance), (Some((3, 2)), 7.0), "on top of the file's defaults");
    }

    #[test]
    fn settings_written_whole_in_a_list_read_back() {
        let c = library(3, 1);
        let (m, text) = saved(&c);
        let whole = serde_json::to_string(&*c.photo(PhotoId(4)).unwrap().develop).unwrap();
        let entry = text.find("\"4\":{\"id\":4,").unwrap();
        let from = entry + text[entry..].find("\"develop\":").unwrap() + "\"develop\":".len();
        let to = from + text[from..].find(",\"edited\":").unwrap();
        m.set(SNAPSHOT, format!("{}[{whole}]{}", &text[..from], &text[to..]).into_bytes());
        let (_, opened, _) = Journal::open(Box::new(m)).unwrap();
        assert!(opened.to_snapshot() == c.to_snapshot());
    }

    #[test]
    fn defaults_or_diffs_that_are_not_settings_are_a_damaged_snapshot() {
        let (m, text) = saved(&library(2, 1));
        for defaults in ["5", "[1,2]", "\"none\"", "{\"light\":\"bright\"}"] {
            m.set(SNAPSHOT, with_defaults(&text, defaults));
            assert!(matches!(Journal::open(Box::new(m.clone())), Err(CatalogError::Corrupt(_))), "defaults {defaults}");
        }
        for develop in ["{\"light\":7}", "[]", "[7]", "[\"x\"]", "[{\"light\":7}]"] {
            m.set(SNAPSHOT, text.replacen("\"develop\":{}", &format!("\"develop\":{develop}"), 1).into_bytes());
            assert!(matches!(Journal::open(Box::new(m.clone())), Err(CatalogError::Corrupt(_))), "develop {develop}");
        }
    }

    #[test]
    fn an_older_whole_snapshot_still_opens_and_is_rewritten_compact() {
        let c = library(4, 2);
        let m = MemStore::new();
        m.set(SNAPSHOT, format!("{{\"format\":\"lightcraft-catalog\",\"version\":12,\"seq\":0,\"catalog\":{}}}\n", c.to_snapshot()).into_bytes());
        let (_, opened, report) = Journal::open(Box::new(m.clone())).unwrap();
        assert_eq!(report.upgraded_from, Some(12));
        assert!(opened.to_snapshot() == c.to_snapshot());
        assert!(String::from_utf8(m.get(SNAPSHOT).unwrap()).unwrap().contains("\"defaults\":{"), "upgraded to the compact form");
    }
}
