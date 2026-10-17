//! What changed, for caches: [`Catalog::revision`] counts every applied op, and
//! [`Catalog::library_revision`] every op except those that only change photos' develop state
//! ([`Op::is_develop_only`]: settings, edit time, History, snapshots). A slider drag applies a
//! develop op per tick; views that don't read develop state (most filters and sorts, the sidebar's
//! counts and trees) key on the library revision and aren't worked out again on every tick.
//!
//! Some views *do* read develop state: "edited" filters and rules, Edit Date, Cropped, Treatment
//! and the size rules (crop and orientation change a photo's shown size). Those key on the full
//! revision ([`Catalog::view_revision`] with `reads_develop`), through the smart albums a filter
//! or rule tests too.

use std::collections::HashSet;

use crate::{AlbumId, Catalog, Filter, Op, Rule, RuleSet, Sort, SortKey};

/// What a cached view of the catalog was worked out from: the library revision, and the full
/// revision as well when the view reads develop state. Equal values mean the view is unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ViewRevision {
    pub library: u64,
    /// [`Catalog::revision`] for views that read develop state; `None` for the others.
    pub all: Option<u64>,
}

/// Smart-album rule fields whose matches don't depend on a photo's develop state. Every other
/// field (including one added later and not listed here) counts as reading it, so a forgotten
/// field costs a query per slider tick, never a stale view.
const FIELDS_WITHOUT_DEVELOP: &[&str] = &[
    "rating",
    "flag",
    "label",
    "text",
    // an album rule reads what that album holds: see `Catalog::filter_reads_develop`
    "album",
    "virtualCopy",
    "copyName",
    "stacked",
    "fileName",
    "extension",
    "filePath",
    "kind",
    "format",
    "duration",
    "captureDate",
    "importDate",
    "keywords",
    "keywordCount",
    "person",
    "personCount",
    "title",
    "caption",
    "altText",
    "creator",
    "copyright",
    "copyrightStatus",
    "camera",
    "lens",
    "focalLength",
    "aperture",
    "shutterSpeed",
    "iso",
    "location",
    // the fork's "Place (from GPS)": the nearest place name of a photo's GPS position
    "place",
    "city",
    "state",
    "country",
    "hasGps",
    "sharpness",
    "bestOfGroup",
];

/// Whether a rule on `field` can match differently after a develop edit (Has Edits, Edit Date,
/// Cropped, Treatment, and the sizes a crop or rotation changes).
pub fn field_reads_develop(field: &str) -> bool {
    !FIELDS_WITHOUT_DEVELOP.contains(&field)
}

impl Op {
    /// Whether the op only changes photos' develop state: their settings and edit time, History
    /// and snapshots. Such ops bump [`Catalog::revision`] but not [`Catalog::library_revision`].
    pub fn is_develop_only(&self) -> bool {
        let mut next = vec![self];
        while let Some(op) = next.pop() {
            match op {
                Op::SetDevelop { .. } | Op::SetHistory { .. } | Op::PushHistory { .. } | Op::SetVersions { .. } => {}
                Op::Batch { ops } => next.extend(ops),
                _ => return false,
            }
        }
        true
    }
}

impl RuleSet {
    /// Whether any of the rules (nested groups included) tests develop state itself; the albums
    /// an Album rule tests are [`Catalog::filter_reads_develop`]'s concern.
    pub fn reads_develop(&self) -> bool {
        let mut next = vec![self];
        while let Some(set) = next.pop() {
            for rule in &set.rules {
                match rule {
                    Rule::Group { group } => next.push(group),
                    Rule::Field { field, .. } if field_reads_develop(field) => return true,
                    Rule::Field { .. } => {}
                }
            }
        }
        false
    }
}

impl Filter {
    /// Whether the filter itself tests develop state: Edited, an `edited:` search token or a rule
    /// that does. The albums it tests are [`Catalog::filter_reads_develop`]'s concern.
    pub fn reads_develop(&self) -> bool {
        self.edited.is_some()
            || self.text.split_whitespace().any(|t| t.to_lowercase().split_once(':').is_some_and(|(field, _)| field == "edited"))
            || self.rule_set.as_ref().is_some_and(RuleSet::reads_develop)
    }
}

impl Sort {
    /// Whether the order depends on develop state (Edit Date: every edit moves the photo).
    pub fn reads_develop(&self) -> bool {
        self.key == SortKey::EditDate
    }
}

impl Catalog {
    /// Whether which photos `filter` lets through can change with a develop edit: the filter's
    /// own fields and rules, and those of every smart album it tests, directly or through other
    /// smart albums (loops in saved rules end the search).
    pub fn filter_reads_develop(&self, filter: &Filter) -> bool {
        if filter.reads_develop() {
            return true;
        }
        self.albums_read_develop(filter.albums_tested())
    }

    /// Whether what album `id` holds can change with a develop edit (a smart album whose rules,
    /// or those of the smart albums they test, read develop state). Plain albums never do.
    pub fn album_reads_develop(&self, id: AlbumId) -> bool {
        self.albums_read_develop(vec![id])
    }

    /// Whether any smart album's contents can change with a develop edit (the sidebar's counts).
    pub fn smart_albums_read_develop(&self) -> bool {
        self.albums_read_develop(self.albums().filter(|a| a.is_smart()).map(|a| a.id).collect())
    }

    fn albums_read_develop(&self, mut next: Vec<AlbumId>) -> bool {
        let mut seen: HashSet<AlbumId> = HashSet::new();
        while let Some(id) = next.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(rules) = self.album(id).and_then(|a| a.smart.as_deref()) else { continue };
            if rules.reads_develop() {
                return true;
            }
            next.extend(rules.albums_tested());
        }
        false
    }

    /// The revision a cached view keys on: the library revision, plus the full revision when the
    /// view reads develop state (see the module docs).
    pub fn view_revision(&self, reads_develop: bool) -> ViewRevision {
        ViewRevision { library: self.library_revision, all: reads_develop.then_some(self.revision) }
    }

    /// Count revisions from `at` (both of them), as a library just loaded does.
    pub fn set_revision(&mut self, at: u64) {
        self.revision = at;
        self.library_revision = at;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::{Album, Photo, PhotoId, Source};

    fn photo(id: u64) -> Photo {
        Photo::new(PhotoId(id), Source::Demo { scene: 0 }, &format!("IMG_{id}.jpg"), "JPEG", 6000, 4000, "2026-01-01T00:00:00")
    }

    fn rules(v: serde_json::Value) -> RuleSet {
        serde_json::from_value(v).unwrap()
    }

    fn smart(id: u64, rules: RuleSet) -> Op {
        let mut a = Album::new(AlbumId(id), format!("Smart {id}"));
        a.smart = Some(Box::new(Filter { rule_set: Some(rules), ..Default::default() }));
        Op::AddAlbum { album: a }
    }

    #[test]
    fn develop_ops_bump_only_the_full_revision() {
        let mut c = Catalog::new();
        c.apply(Op::AddPhoto { photo: Box::new(photo(1)) }).unwrap();
        let (rev, lib) = (c.revision, c.library_revision);
        let mut s = (*c.photo(PhotoId(1)).unwrap().develop).clone();
        s.light.exposure = 1.0;
        let set =
            Op::SetDevelop { id: PhotoId(1), settings: Arc::new(s.clone()), label: "Exposure".into(), edited: Some("2026-02-01T00:00:00".into()) };
        let step = Op::PushHistory { id: PhotoId(1), step: crate::HistoryStep { label: "Exposure".into(), settings: Arc::new(s) } };
        c.apply(Op::Batch { ops: vec![set, step] }).unwrap();
        assert_eq!((c.revision, c.library_revision), (rev + 1, lib), "a develop edit leaves the library revision");
        c.apply(Op::SetRating { id: PhotoId(1), rating: 3 }).unwrap();
        assert_eq!((c.revision, c.library_revision), (rev + 2, lib + 1));
        // a batch with any library op is a library change
        let mixed =
            Op::Batch { ops: vec![Op::SetHistory { id: PhotoId(1), history: Vec::new() }, Op::SetFlag { id: PhotoId(1), flag: crate::Flag::Pick }] };
        c.apply(mixed).unwrap();
        assert_eq!(c.library_revision, lib + 2);
        // a failed op changes neither
        assert!(c.apply(Op::SetRating { id: PhotoId(9), rating: 3 }).is_err());
        assert_eq!((c.revision, c.library_revision), (rev + 3, lib + 2));
    }

    #[test]
    fn filters_sorts_and_rules_that_read_develop_state() {
        assert!(!Filter::default().reads_develop());
        assert!(!Filter { text: "harbour rating:3".into(), rating: 2, keyword: Some("travel".into()), ..Default::default() }.reads_develop());
        assert!(Filter { edited: Some(false), ..Default::default() }.reads_develop());
        assert!(Filter { text: "harbour Edited:yes".into(), ..Default::default() }.reads_develop());
        assert!(Sort { key: SortKey::EditDate, ..Default::default() }.reads_develop());
        assert!(!Sort { key: SortKey::FileName, ..Default::default() }.reads_develop());
        for field in ["edited", "cropped", "treatment", "editDate", "longEdge", "shortEdge", "aspect", "megapixels"] {
            let nested = rules(
                json!({"rules": [{"field": "rating", "op": ">=", "value": 1}, {"group": {"match": "any", "rules": [{"field": field, "op": "is", "value": true}]}}]}),
            );
            assert!(nested.reads_develop(), "{field}");
        }
        assert!(
            !rules(json!({"rules": [{"field": "rating", "op": ">=", "value": 1}, {"field": "keywords", "op": "contains", "value": "x"}]}))
                .reads_develop()
        );
    }

    /// Every rule field is either known not to read develop state or counts as reading it: a
    /// new field must be put on one side on purpose.
    #[test]
    fn every_rule_field_is_classified() {
        let reads: Vec<&str> = crate::rules::FIELDS.iter().map(|f| f.0).filter(|f| field_reads_develop(f)).collect();
        assert_eq!(reads, ["editDate", "longEdge", "shortEdge", "aspect", "megapixels", "edited", "cropped", "treatment"]);
        for f in FIELDS_WITHOUT_DEVELOP {
            assert!(crate::rules::FIELDS.iter().any(|x| x.0 == *f), "{f} is not a rule field");
        }
    }

    #[test]
    fn smart_albums_read_develop_through_the_albums_they_test() {
        let mut c = Catalog::new();
        c.apply(smart(1, rules(json!({"rules": [{"field": "rating", "op": ">=", "value": 3}]})))).unwrap();
        c.apply(smart(2, rules(json!({"rules": [{"field": "edited", "op": "is", "value": true}]})))).unwrap();
        // 3 tests 2 (which reads develop); 4 and 5 test each other (a loop) and 1
        c.apply(smart(3, rules(json!({"rules": [{"field": "album", "op": "is", "value": 2}]})))).unwrap();
        c.apply(smart(4, rules(json!({"rules": [{"field": "album", "op": "is", "value": 5}, {"field": "album", "op": "is", "value": 1}]})))).unwrap();
        c.apply(smart(5, rules(json!({"rules": [{"field": "album", "op": "is", "value": 4}]})))).unwrap();
        c.apply(Op::AddAlbum { album: Album::new(AlbumId(6), "Plain") }).unwrap();
        let reads = |id: u64| c.album_reads_develop(AlbumId(id));
        assert_eq!([1, 2, 3, 4, 5, 6, 7].map(reads), [false, true, true, false, false, false, false]);
        assert!(c.filter_reads_develop(&Filter { album: Some(AlbumId(3)), ..Default::default() }));
        assert!(!c.filter_reads_develop(&Filter { album: Some(AlbumId(4)), ..Default::default() }));
        assert!(c.smart_albums_read_develop());
        c.apply(Op::RemoveAlbum { id: AlbumId(2) }).unwrap();
        c.apply(Op::RemoveAlbum { id: AlbumId(3) }).unwrap();
        assert!(!c.smart_albums_read_develop());
    }
}
